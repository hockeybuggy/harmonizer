use super::{
    check,
    interfaces::{Automation, Events, Handler},
    keep, param_id,
};
use anyhow::{Context, Result, bail, ensure};
use std::{f64::consts::TAU, mem, ptr};
use vst3::{
    ComPtr, ComWrapper,
    Steinberg::Vst::{
        BusDirections_::{kInput, kOutput},
        BusTypes_::{kAux, kMain},
        Event_::EventTypes_::{kNoteOffEvent, kNoteOnEvent},
        MediaTypes_::{kAudio, kEvent},
        ProcessModes_::kRealtime,
        RestartFlags_::kLatencyChanged,
        SymbolicSampleSizes_::kSample32,
        *,
    },
};

const RATE: usize = 48_000;
const BLOCK: usize = 512;
const NOTE_ON: usize = 10 * BLOCK + 100;
const NOTE_OFF: usize = NOTE_ON + RATE;
const AUTOMATE: usize = 75 * BLOCK;
const BUSES: [(MediaType, BusDirection); 3] = [
    (kAudio as MediaType, kInput as BusDirection),
    (kAudio as MediaType, kOutput as BusDirection),
    (kEvent as MediaType, kInput as BusDirection),
];

pub(super) fn check_audio(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    handler: &Handler,
    instrument: bool,
    allow_aux_workaround: bool,
) -> Result<bool> {
    let used_workaround = negotiate(component, processor, instrument, allow_aux_workaround)?;
    let mut active_buses = 0;
    let result = (|| {
        for (media, direction) in BUSES {
            check(
                unsafe { component.activateBus(media, direction, 0, 1) },
                "activateBus",
            )?;
            active_buses += 1;
        }
        check(
            unsafe { processor.canProcessSampleSize(kSample32 as i32) },
            "canProcessSampleSize(kSample32)",
        )?;
        let mut setup = ProcessSetup {
            processMode: kRealtime as i32,
            symbolicSampleSize: kSample32 as i32,
            maxSamplesPerBlock: BLOCK as i32,
            sampleRate: RATE as f64,
        };
        check(
            unsafe { processor.setupProcessing(&mut setup) },
            "setupProcessing",
        )?;
        check(unsafe { component.setActive(1) }, "setActive(true)")?;
        let result = (|| {
            check(unsafe { processor.setProcessing(1) }, "setProcessing(true)")?;
            let result = render(processor, handler);
            keep(
                result,
                check(
                    unsafe { processor.setProcessing(0) },
                    "setProcessing(false)",
                ),
            )
        })();
        keep(
            result,
            check(unsafe { component.setActive(0) }, "setActive(false)"),
        )
    })();
    let mut result = result;
    for &(media, direction) in BUSES[..active_buses].iter().rev() {
        result = keep(
            result,
            check(
                unsafe { component.activateBus(media, direction, 0, 0) },
                "deactivate bus",
            ),
        );
    }
    result.map(|()| used_workaround)
}

fn negotiate(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    instrument: bool,
    allow_aux_workaround: bool,
) -> Result<bool> {
    for (media, dir) in BUSES {
        let count = unsafe { component.getBusCount(media, dir) };
        ensure!(count == 1, "expected one {media}/{dir} bus, found {count}");
    }
    for (index, (media, dir)) in BUSES[..2].iter().copied().enumerate() {
        let mut info: BusInfo = unsafe { mem::zeroed() };
        check(
            unsafe { component.getBusInfo(media, dir, 0, &mut info) },
            "getBusInfo",
        )?;
        let aux = index == 0 && instrument;
        let expected = if aux { kAux } else { kMain } as BusType;
        ensure!(
            info.busType == expected && info.channelCount == 2,
            "expected a stereo {} bus for audio direction {dir}; got bus type {}, {} channels",
            if aux { "aux" } else { "main" },
            info.busType,
            info.channelCount
        );
        println!(
            "  Audio {}: stereo {}",
            if index == 0 { "input" } else { "output" },
            if aux { "aux" } else { "main" }
        );
    }

    // nice-plug 0.4.2 reads past the advertised input count on aux-only layouts.
    let mut input = [SpeakerArr::kStereo, SpeakerArr::kEmpty];
    let mut output = SpeakerArr::kStereo;
    let result = check(
        unsafe { processor.setBusArrangements(input.as_mut_ptr(), 1, &mut output, 1) },
        "setBusArrangements(stereo)",
    );
    if result.is_ok() || !instrument {
        return result.map(|()| false);
    }
    if !allow_aux_workaround {
        return result.context("the instrument rejected its sole stereo aux input; --allow-aux-workaround can run the remaining checks").map(|()| false);
    }
    input[1] = SpeakerArr::kStereo;
    check(
        unsafe { processor.setBusArrangements(input.as_mut_ptr(), 1, &mut output, 1) },
        "setBusArrangements with padded aux workaround",
    )?;
    eprintln!(
        "  WARNING: rejected the advertised stereo aux arrangement; accepted only with a padded second stereo entry (nice-plug aux-only workaround)"
    );
    Ok(true)
}

fn render(processor: &ComPtr<IAudioProcessor>, handler: &Handler) -> Result<()> {
    let latency = unsafe { processor.getLatencySamples() } as usize;
    ensure!(latency > 0, "reported zero latency after activation");
    let _ = handler.take_flags();

    let on = note_events(NOTE_ON, true);
    let off = note_events(NOTE_OFF, false);
    let on = on.as_com_ref::<IEventList>().unwrap();
    let off = off.as_com_ref::<IEventList>().unwrap();
    let automation = ComWrapper::new(Automation::new(param_id("mode"), param_id("quality")));
    let automation = automation.as_com_ref::<IParameterChanges>().unwrap();
    let [mut before, mut held, mut changed, mut tail] = [Level::default(); 4];
    let mut new_latency = latency;
    let mut input = [[0.0_f32; BLOCK]; 2];
    let mut output = [[0.0_f32; BLOCK]; 2];

    for start in (0..3 * RATE).step_by(BLOCK) {
        let len = BLOCK.min(3 * RATE - start);
        for i in 0..len {
            let sample = (TAU * 220.0 * (start + i) as f64 / RATE as f64).sin() as f32 * 0.5;
            for channel in &mut input {
                channel[i] = sample;
            }
        }
        for channel in &mut output {
            channel[..len].fill(f32::NAN);
        }
        let mut in_channels = input.each_mut().map(|ch| ch.as_mut_ptr());
        let mut out_channels = output.each_mut().map(|ch| ch.as_mut_ptr());
        let mut inputs = stereo_buffer(&mut in_channels);
        let mut outputs = stereo_buffer(&mut out_channels);
        let events = if start == NOTE_ON / BLOCK * BLOCK {
            on.as_ptr()
        } else if start == NOTE_OFF / BLOCK * BLOCK {
            off.as_ptr()
        } else {
            ptr::null_mut()
        };
        let mut data = ProcessData {
            processMode: kRealtime as i32,
            symbolicSampleSize: kSample32 as i32,
            numSamples: len as i32,
            numInputs: 1,
            numOutputs: 1,
            inputs: &mut inputs,
            outputs: &mut outputs,
            inputParameterChanges: if start == AUTOMATE {
                automation.as_ptr()
            } else {
                ptr::null_mut()
            },
            outputParameterChanges: ptr::null_mut(),
            inputEvents: events,
            outputEvents: ptr::null_mut(),
            processContext: ptr::null_mut(),
        };
        check(unsafe { processor.process(&mut data) }, "process")
            .with_context(|| format!("sample {start}"))?;
        if start == AUTOMATE {
            new_latency = unsafe { processor.getLatencySamples() } as usize;
            let flags = handler.take_flags();
            ensure!(
                new_latency != latency && new_latency > 0,
                "latency did not change to a positive value after automation ({latency} → {new_latency})"
            );
            ensure!(
                flags & kLatencyChanged != 0,
                "latency changed ({latency} → {new_latency}) without restartComponent(kLatencyChanged); flags {flags:#x}"
            );
            println!("  Latency: {latency} → {new_latency} samples; received kLatencyChanged");
        }

        for (channel, samples) in output.iter().enumerate() {
            for (i, &sample) in samples[..len].iter().enumerate() {
                let index = start + i;
                if !sample.is_finite() {
                    bail!("non-finite output on channel {channel} at sample {index}: {sample}");
                }
                if index < NOTE_ON {
                    before.push(sample);
                } else if (NOTE_ON + latency + RATE / 4..AUTOMATE).contains(&index) {
                    held.push(sample);
                } else if (AUTOMATE + new_latency + RATE / 10..NOTE_OFF).contains(&index) {
                    changed.push(sample);
                } else if index >= NOTE_OFF + new_latency + RATE / 2 {
                    tail.push(sample);
                }
            }
        }
    }

    let before = before.rms("before Note On")?;
    let held = held.rms("held notes before automation")?;
    let changed = changed.rms("held notes after automation")?;
    let tail = tail.rms("after release")?;
    println!(
        "  Audio RMS: before={before:.3e}, held={held:.3e}, after automation={changed:.3e}, after release={tail:.3e}"
    );
    ensure!(
        before < 1e-5,
        "expected silence before Note On; RMS {before:.3e}"
    );
    ensure!(held > 0.01, "expected audible held notes; RMS {held:.3e}");
    ensure!(
        changed > 0.01,
        "expected audible notes after automation; RMS {changed:.3e}"
    );
    ensure!(
        tail < 1e-5,
        "expected silence after release; RMS {tail:.3e}"
    );
    Ok(())
}

fn stereo_buffer(channels: &mut [*mut f32; 2]) -> AudioBusBuffers {
    AudioBusBuffers {
        numChannels: 2,
        silenceFlags: 0,
        __field0: AudioBusBuffers__type0 {
            channelBuffers32: channels.as_mut_ptr(),
        },
    }
}

fn note_events(at: usize, on: bool) -> ComWrapper<Events> {
    ComWrapper::new(Events([57, 64].map(|pitch| {
        let note_id = i32::from(pitch);
        Event {
            busIndex: 0,
            sampleOffset: (at % BLOCK) as i32,
            ppqPosition: 2.0 * at as f64 / RATE as f64,
            flags: 0,
            r#type: if on { kNoteOnEvent } else { kNoteOffEvent } as u16,
            __field0: if on {
                Event__type0 {
                    noteOn: NoteOnEvent {
                        channel: 0,
                        pitch,
                        tuning: 0.0,
                        velocity: 0.9,
                        length: 0,
                        noteId: note_id,
                    },
                }
            } else {
                Event__type0 {
                    noteOff: NoteOffEvent {
                        channel: 0,
                        pitch,
                        tuning: 0.0,
                        velocity: 0.0,
                        noteId: note_id,
                    },
                }
            },
        }
    })))
}

#[derive(Clone, Copy, Default)]
struct Level {
    squares: f64,
    samples: usize,
}

impl Level {
    fn push(&mut self, sample: f32) {
        self.squares += f64::from(sample).powi(2);
        self.samples += 1;
    }

    fn rms(&self, label: &str) -> Result<f64> {
        ensure!(
            self.samples > 0,
            "no output samples in the {label} measurement window"
        );
        Ok((self.squares / self.samples as f64).sqrt())
    }
}
