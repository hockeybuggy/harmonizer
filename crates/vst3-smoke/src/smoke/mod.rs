mod interfaces;
mod processing;

use anyhow::{Context, Result, bail, ensure};
use core_foundation::{
    base::TCFType,
    bundle::{CFBundle, CFBundleRef},
    url::CFURL,
};
use interfaces::{Handler, Host, StateStream};
use libloading::Library;
use std::{
    env,
    ffi::c_char,
    mem,
    path::{Path, PathBuf},
    ptr,
};
use vst3::{
    ComPtr, ComWrapper, Interface,
    Steinberg::{Vst::*, *},
};

const EFFECT: &str = "Harmonizer";
const INSTRUMENT: &str = "Harmonizer Instrument";
const PARAMETERS: [(&str, &str); 11] = [
    ("mode", "Mode"),
    ("quality", "Quality"),
    ("wet", "Wet"),
    ("dry", "Dry"),
    ("formant", "Formant"),
    ("refnote", "Reference Note"),
    ("bendrange", "Bend Range"),
    ("attack", "Attack"),
    ("release", "Release"),
    ("velsens", "Velocity"),
    ("spread", "Spread"),
];

pub fn run() -> Result<()> {
    let usage = "usage: vst3-smoke <Harmonizer.vst3> [--allow-aux-workaround]";
    let mut args = env::args_os().skip(1);
    let first = args.next().context(usage)?;
    if first == "--help" {
        println!("{usage}");
        return Ok(());
    }
    let allow_aux_workaround = match args.next() {
        None => false,
        Some(flag) if flag == "--allow-aux-workaround" && args.next().is_none() => true,
        _ => bail!(usage),
    };
    let path = PathBuf::from(first);
    let bundle = path
        .canonicalize()
        .with_context(|| format!("locate bundle {}", path.display()))?;
    let used_workaround = check_bundle(&bundle, allow_aux_workaround)?;
    if used_workaround {
        println!("PASS: both classes (with the Instrument aux arrangement workaround)");
    } else {
        println!("PASS: both classes");
    }
    Ok(())
}

fn check_bundle(path: &Path, allow_aux_workaround: bool) -> Result<bool> {
    type Entry = unsafe extern "C" fn(CFBundleRef) -> bool;
    type Exit = unsafe extern "C" fn() -> bool;
    type GetFactory = unsafe extern "system" fn() -> *mut IPluginFactory;

    let url = CFURL::from_path(path, true).context("create bundle URL")?;
    let bundle = CFBundle::new(url).context("create CFBundle")?;
    let executable = path.join("Contents/MacOS/Harmonizer");
    let library = unsafe { Library::new(&executable) }
        .with_context(|| format!("load {}", executable.display()))?;
    let entry = unsafe { library.get::<Entry>(b"bundleEntry\0") }.context("find bundleEntry")?;
    let exit = unsafe { library.get::<Exit>(b"bundleExit\0") }.context("find bundleExit")?;
    let get_factory = unsafe { library.get::<GetFactory>(b"GetPluginFactory\0") }
        .context("find GetPluginFactory")?;
    ensure!(
        unsafe { entry(bundle.as_concrete_TypeRef()) },
        "bundleEntry returned false"
    );

    // All plug-in interfaces must be released before bundleExit and dlclose.
    let result = match unsafe { ComPtr::from_raw(get_factory()) } {
        Some(factory) => check_factory(&factory, allow_aux_workaround),
        None => Err(anyhow::anyhow!("GetPluginFactory returned null")),
    };
    let exited = unsafe { exit() };
    let used_workaround = result?;
    ensure!(exited, "bundleExit returned false");
    Ok(used_workaround)
}

fn check_factory(factory: &ComPtr<IPluginFactory>, allow_aux_workaround: bool) -> Result<bool> {
    let mut classes = Vec::new();
    for index in 0..unsafe { factory.countClasses() } {
        let mut info: PClassInfo = unsafe { mem::zeroed() };
        check(
            unsafe { factory.getClassInfo(index, &mut info) },
            "getClassInfo",
        )?;
        let name = decode_c_field(&info.name);
        let category = decode_c_field(&info.category);
        println!("Class {index}: {name} ({category})");
        if category == "Audio Module Class" {
            classes.push((name, info.cid));
        }
    }
    classes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let names: Vec<_> = classes.iter().map(|(name, _)| name.as_str()).collect();
    ensure!(
        names == [EFFECT, INSTRUMENT],
        "expected exactly {EFFECT:?} and {INSTRUMENT:?} audio classes, got {names:?}"
    );

    let host = ComWrapper::new(Host);
    let context = host
        .as_com_ref::<IHostApplication>()
        .unwrap()
        .upcast::<FUnknown>();
    let mut used_workaround = false;
    for (name, cid) in classes {
        println!("{name}:");
        let instrument = name == INSTRUMENT;
        let result = check_class(
            factory,
            &cid,
            context.as_ptr(),
            instrument,
            allow_aux_workaround,
        )
        .with_context(|| name)?;
        used_workaround |= result;
    }
    Ok(used_workaround)
}

fn check_class(
    factory: &ComPtr<IPluginFactory>,
    cid: &TUID,
    context: *mut FUnknown,
    instrument: bool,
    allow_aux_workaround: bool,
) -> Result<bool> {
    let component = create::<IComponent>(factory, cid, "IComponent")?;
    check(
        unsafe { component.initialize(context) },
        "IComponent::initialize",
    )?;
    let result = (|| {
        let processor = component
            .cast::<IAudioProcessor>()
            .context("missing IAudioProcessor")?;
        if let Some(controller) = component.cast::<IEditController>() {
            exercise(
                &component,
                &processor,
                &controller,
                instrument,
                allow_aux_workaround,
            )
        } else {
            let mut controller_cid = [0; 16];
            check(
                unsafe { component.getControllerClassId(&mut controller_cid) },
                "getControllerClassId",
            )?;
            let controller =
                create::<IEditController>(factory, &controller_cid, "IEditController")?;
            check(
                unsafe { controller.initialize(context) },
                "IEditController::initialize",
            )?;
            let result = exercise(
                &component,
                &processor,
                &controller,
                instrument,
                allow_aux_workaround,
            );
            keep(
                result,
                check(
                    unsafe { controller.terminate() },
                    "IEditController::terminate",
                ),
            )
        }
    })();
    keep(
        result,
        check(unsafe { component.terminate() }, "IComponent::terminate"),
    )
}

fn create<I: Interface>(
    factory: &ComPtr<IPluginFactory>,
    cid: &TUID,
    label: &str,
) -> Result<ComPtr<I>> {
    let mut instance = ptr::null_mut();
    check(
        unsafe { factory.createInstance(cid.as_ptr(), I::IID.as_ptr().cast(), &mut instance) },
        "createInstance",
    )
    .with_context(|| format!("create {label}"))?;
    unsafe { ComPtr::from_raw(instance.cast()) }
        .with_context(|| format!("factory returned null {label}"))
}

fn exercise(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    controller: &ComPtr<IEditController>,
    instrument: bool,
    allow_aux_workaround: bool,
) -> Result<bool> {
    check_parameters(controller)?;
    let handler = ComWrapper::new(Handler::default());
    let handler_ptr = handler.as_com_ref::<IComponentHandler>().unwrap();
    check(
        unsafe { controller.setComponentHandler(handler_ptr.as_ptr()) },
        "setComponentHandler",
    )?;
    let result = (|| {
        let used_workaround = processing::check_audio(
            component,
            processor,
            &handler,
            instrument,
            allow_aux_workaround,
        )?;
        check_state(component, controller)?;
        let view = unsafe { ComPtr::from_raw(controller.createView(ViewType::kEditor)) }
            .context("createView(editor) returned null")?;
        check(
            unsafe { view.isPlatformTypeSupported(kPlatformTypeNSView) },
            "editor does not support NSView",
        )?;
        println!("  Editor: NSView supported");
        Ok(used_workaround)
    })();
    keep(
        result,
        check(
            unsafe { controller.setComponentHandler(ptr::null_mut()) },
            "clear component handler",
        ),
    )
}

fn check_parameters(controller: &ComPtr<IEditController>) -> Result<()> {
    use ParameterInfo_::ParameterFlags_::kIsHidden;

    let count = unsafe { controller.getParameterCount() };
    ensure!(
        count >= PARAMETERS.len() as i32,
        "expected at least {} parameters, got {count}",
        PARAMETERS.len()
    );
    println!("  Parameters: {count}");
    let mut seen = [false; PARAMETERS.len()];
    let mut hidden = 0;
    for index in 0..count {
        let mut info: ParameterInfo = unsafe { mem::zeroed() };
        check(
            unsafe { controller.getParameterInfo(index, &mut info) },
            "getParameterInfo",
        )?;
        let name = decode_utf16(&info.title);
        if let Some(slot) = PARAMETERS
            .iter()
            .position(|(key, _)| param_id(key) == info.id)
        {
            let (key, expected) = PARAMETERS[slot];
            ensure!(
                name == expected,
                "parameter {key}: expected {expected:?}, got {name:?}"
            );
            ensure!(!seen[slot], "duplicate parameter {key}");
            seen[slot] = true;
            println!("    {name} ({key}, {:#010x})", info.id);
        } else if info.flags & kIsHidden != 0 {
            hidden += 1;
        } else {
            println!("    {name} ({:#010x})", info.id);
        }
    }
    if hidden != 0 {
        println!("    … {hidden} hidden parameters");
    }
    for (present, (key, name)) in seen.into_iter().zip(PARAMETERS) {
        ensure!(
            present,
            "missing parameter {name} ({key}, {:#010x})",
            param_id(key)
        );
    }
    Ok(())
}

fn check_state(component: &ComPtr<IComponent>, controller: &ComPtr<IEditController>) -> Result<()> {
    let ids = [param_id("mode"), param_id("quality")];
    for id in ids {
        ensure!(
            unsafe { controller.getParamNormalized(id) } == 1.0,
            "automation did not set parameter {id:#010x} to 1"
        );
    }
    let stream = ComWrapper::new(StateStream::default());
    let ptr = stream.as_com_ref::<IBStream>().unwrap().as_ptr();
    check(unsafe { component.getState(ptr) }, "IComponent::getState")?;
    ensure!(stream.len() > 0, "getState returned an empty stream");
    for id in ids {
        check(
            unsafe { controller.setParamNormalized(id, 0.0) },
            "mutate parameter before setState",
        )?;
        ensure!(
            unsafe { controller.getParamNormalized(id) } == 0.0,
            "parameter mutation was ignored for {id:#010x}"
        );
    }
    stream.rewind();
    check(unsafe { component.setState(ptr) }, "IComponent::setState")?;
    stream.rewind();
    check(
        unsafe { controller.setComponentState(ptr) },
        "IEditController::setComponentState",
    )?;
    for id in ids {
        ensure!(
            unsafe { controller.getParamNormalized(id) } == 1.0,
            "state did not restore parameter {id:#010x}"
        );
    }
    println!("  State: {} bytes; Mode and Quality restored", stream.len());
    Ok(())
}

fn check(result: tresult, operation: &str) -> Result<()> {
    ensure!(
        result == kResultOk,
        "{operation} failed (VST3 result {result})"
    );
    Ok(())
}

fn keep<T>(result: Result<T>, cleanup: Result<()>) -> Result<T> {
    let value = result?;
    cleanup?;
    Ok(value)
}

fn param_id(key: &str) -> ParamID {
    key.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    }) & 0x7fff_ffff
}

fn decode_c_field(field: &[c_char]) -> String {
    let bytes: Vec<_> = field
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn decode_utf16(field: &[u16]) -> String {
    let len = field
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(field.len());
    String::from_utf16_lossy(&field[..len])
}
