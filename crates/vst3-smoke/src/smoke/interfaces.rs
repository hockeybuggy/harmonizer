use std::{
    cell::RefCell,
    ffi::c_void,
    io::{Cursor, Seek, SeekFrom, Write},
    ptr, slice,
    sync::atomic::{AtomicI32, Ordering},
};
use vst3::{
    Class, ComPtr, ComWrapper,
    Steinberg::{Vst::*, *},
};

pub(super) struct Host;

impl Class for Host {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for Host {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        let mut text = [0; 128];
        for (dst, unit) in text
            .iter_mut()
            .zip("Harmonizer VST3 Smoke Host".encode_utf16())
        {
            *dst = unit;
        }
        unsafe { name.write(text) };
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _: *mut TUID,
        _: *mut TUID,
        object: *mut *mut c_void,
    ) -> tresult {
        if !object.is_null() {
            unsafe { object.write(ptr::null_mut()) };
        }
        kNoInterface
    }
}

#[derive(Default)]
pub(super) struct Handler(AtomicI32);

impl Handler {
    pub fn take_flags(&self) -> i32 {
        self.0.swap(0, Ordering::Relaxed)
    }
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, _: ParamID, _: ParamValue) -> tresult {
        kResultOk
    }

    unsafe fn endEdit(&self, _: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: i32) -> tresult {
        self.0.fetch_or(flags, Ordering::Relaxed);
        kResultOk
    }
}

pub(super) struct Events(pub [Event; 2]);

impl Class for Events {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for Events {
    unsafe fn getEventCount(&self) -> i32 {
        self.0.len() as i32
    }

    unsafe fn getEvent(&self, index: i32, event: *mut Event) -> tresult {
        match self.0.get(index as usize) {
            Some(&value) if !event.is_null() => {
                unsafe { event.write(value) };
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, _: *mut Event) -> tresult {
        kNotImplemented
    }
}

struct ParamPoint(ParamID, ParamValue);

impl Class for ParamPoint {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamPoint {
    unsafe fn getParameterId(&self) -> ParamID {
        self.0
    }

    unsafe fn getPointCount(&self) -> i32 {
        1
    }

    unsafe fn getPoint(&self, index: i32, offset: *mut i32, value: *mut ParamValue) -> tresult {
        if index != 0 || offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            offset.write(0);
            value.write(self.1);
        }
        kResultOk
    }

    unsafe fn addPoint(&self, _: i32, _: ParamValue, _: *mut i32) -> tresult {
        kNotImplemented
    }
}

pub(super) struct Automation([ComPtr<IParamValueQueue>; 2]);

impl Automation {
    pub fn new(mode: ParamID, quality: ParamID) -> Self {
        Self([mode, quality].map(|id| ComWrapper::new(ParamPoint(id, 1.0)).to_com_ptr().unwrap()))
    }
}

impl Class for Automation {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for Automation {
    unsafe fn getParameterCount(&self) -> i32 {
        self.0.len() as i32
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        self.0
            .get(index as usize)
            .map_or(ptr::null_mut(), ComPtr::as_ptr)
    }

    unsafe fn addParameterData(&self, _: *const ParamID, _: *mut i32) -> *mut IParamValueQueue {
        ptr::null_mut()
    }
}

#[derive(Default)]
pub(super) struct StateStream(RefCell<Cursor<Vec<u8>>>);

impl StateStream {
    pub fn len(&self) -> usize {
        self.0.borrow().get_ref().len()
    }

    pub fn rewind(&self) {
        self.0.borrow_mut().set_position(0);
    }
}

impl Class for StateStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for StateStream {
    unsafe fn read(&self, buffer: *mut c_void, size: i32, read: *mut i32) -> tresult {
        if size < 0 || (size > 0 && buffer.is_null()) {
            return kInvalidArgument;
        }
        let mut cursor = self.0.borrow_mut();
        let position = cursor.position();
        let data = cursor
            .get_ref()
            .get(position as usize..)
            .unwrap_or_default();
        let count = (size as usize).min(data.len());
        if count > 0 {
            unsafe { ptr::copy_nonoverlapping(data.as_ptr(), buffer.cast(), count) };
            cursor.set_position(position + count as u64);
        }
        if !read.is_null() {
            unsafe { read.write(count as i32) };
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, size: i32, written: *mut i32) -> tresult {
        if size < 0 || (size > 0 && buffer.is_null()) {
            return kInvalidArgument;
        }
        let bytes = if size == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(buffer.cast(), size as usize) }
        };
        if self.0.borrow_mut().write_all(bytes).is_err() {
            return kResultFalse;
        }
        if !written.is_null() {
            unsafe { written.write(size) };
        }
        kResultOk
    }

    unsafe fn seek(&self, offset: i64, mode: i32, position: *mut i64) -> tresult {
        use IBStream_::{
            IStreamSeekMode,
            IStreamSeekMode_::{kIBSeekCur as CURRENT, kIBSeekEnd as END, kIBSeekSet as START},
        };
        let from = match mode as IStreamSeekMode {
            START if offset >= 0 => SeekFrom::Start(offset as u64),
            CURRENT => SeekFrom::Current(offset),
            END => SeekFrom::End(offset),
            _ => return kInvalidArgument,
        };
        match self.0.borrow_mut().seek(from) {
            Ok(new_position) => {
                if !position.is_null() {
                    unsafe { position.write(new_position as i64) };
                }
                kResultOk
            }
            Err(_) => kInvalidArgument,
        }
    }

    unsafe fn tell(&self, position: *mut i64) -> tresult {
        if position.is_null() {
            return kInvalidArgument;
        }
        unsafe { position.write(self.0.borrow().position() as i64) };
        kResultOk
    }
}
