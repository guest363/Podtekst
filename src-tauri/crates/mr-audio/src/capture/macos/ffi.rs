use std::ffi::{c_char, c_void};

pub(super) type OSStatus = i32;
pub(super) type AudioDeviceId = u32;
pub(super) type AudioObjectId = u32;
pub(super) type AudioUnit = *mut c_void;
pub(super) type AudioComponent = *mut c_void;

#[repr(C)]
pub(super) struct AudioComponentDescription {
    pub(super) component_type: u32,
    pub(super) component_sub_type: u32,
    pub(super) component_manufacturer: u32,
    pub(super) component_flags: u32,
    pub(super) component_flags_mask: u32,
}

#[repr(C)]
pub(super) struct AudioStreamBasicDescription {
    pub(super) sample_rate: f64,
    pub(super) format_id: u32,
    pub(super) format_flags: u32,
    pub(super) bytes_per_packet: u32,
    pub(super) frames_per_packet: u32,
    pub(super) bytes_per_frame: u32,
    pub(super) channels_per_frame: u32,
    pub(super) bits_per_channel: u32,
    pub(super) reserved: u32,
}

#[repr(C)]
pub(super) struct AudioObjectPropertyAddress {
    pub(super) selector: u32,
    pub(super) scope: u32,
    pub(super) element: u32,
}

#[repr(C)]
pub(super) struct AudioBuffer {
    pub(super) number_channels: u32,
    pub(super) data_byte_size: u32,
    pub(super) data: *mut c_void,
}

#[repr(C)]
pub(super) struct AudioBufferList {
    pub(super) number_buffers: u32,
    pub(super) buffers: [AudioBuffer; 1],
}

/// Непрозрачная метка времени CoreAudio: колбэк получает указатель и передаёт
/// его в `AudioUnitRender`, поля не нужны.
#[repr(C)]
pub(super) struct AudioTimeStamp {
    pub(super) _private: [u8; 0],
}

pub(super) type RenderCallback = unsafe extern "C" fn(
    *mut c_void,
    *mut u32,
    *const AudioTimeStamp,
    u32,
    u32,
    *mut AudioBufferList,
) -> OSStatus;

#[repr(C)]
pub(super) struct RenderCallbackStruct {
    pub(super) input_proc: Option<RenderCallback>,
    pub(super) input_proc_ref_con: *mut c_void,
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    pub(super) fn AudioComponentFindNext(
        component: AudioComponent,
        desc: *const AudioComponentDescription,
    ) -> AudioComponent;
    pub(super) fn AudioComponentInstanceNew(
        component: AudioComponent,
        unit: *mut AudioUnit,
    ) -> OSStatus;
    pub(super) fn AudioComponentInstanceDispose(unit: AudioUnit) -> OSStatus;
    pub(super) fn AudioUnitSetProperty(
        unit: AudioUnit,
        property: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> OSStatus;
    pub(super) fn AudioUnitGetProperty(
        unit: AudioUnit,
        property: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> OSStatus;
    pub(super) fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    pub(super) fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    pub(super) fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
    pub(super) fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
    pub(super) fn AudioUnitRender(
        unit: AudioUnit,
        action_flags: *mut u32,
        timestamp: *const AudioTimeStamp,
        bus: u32,
        frames: u32,
        data: *mut AudioBufferList,
    ) -> OSStatus;
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    pub(super) fn AudioObjectGetPropertyDataSize(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> OSStatus;
    pub(super) fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub(super) fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> u8;
    pub(super) fn CFRelease(value: *const c_void);
}

pub(super) const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

pub(super) const UNIT_TYPE_OUTPUT: u32 = fourcc(b"auou");
pub(super) const UNIT_SUBTYPE_HAL_OUTPUT: u32 = fourcc(b"ahal");
pub(super) const UNIT_MANUFACTURER_APPLE: u32 = fourcc(b"appl");
pub(super) const UNIT_SCOPE_GLOBAL: u32 = 0;
pub(super) const UNIT_SCOPE_INPUT: u32 = 1;
pub(super) const UNIT_SCOPE_OUTPUT: u32 = 2;

pub(super) const OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE: u32 = 2000;
pub(super) const OUTPUT_UNIT_PROPERTY_ENABLE_IO: u32 = 2003;
pub(super) const OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK: u32 = 2005;
pub(super) const UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;

pub(super) const FORMAT_LINEAR_PCM: u32 = fourcc(b"lpcm");
pub(super) const FORMAT_FLAG_IS_FLOAT: u32 = 1 << 0;
pub(super) const FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;

pub(super) const OBJECT_SYSTEM: u32 = 1;
pub(super) const OBJECT_SCOPE_GLOBAL: u32 = fourcc(b"glob");
pub(super) const OBJECT_SCOPE_INPUT: u32 = fourcc(b"inpt");
pub(super) const OBJECT_ELEMENT_MAIN: u32 = 0;
pub(super) const HARDWARE_PROPERTY_DEVICES: u32 = fourcc(b"dev#");
pub(super) const HARDWARE_PROPERTY_DEFAULT_INPUT: u32 = fourcc(b"dIn ");
pub(super) const DEVICE_PROPERTY_STREAM_CONFIGURATION: u32 = fourcc(b"slay");
pub(super) const DEVICE_PROPERTY_NAME_CFSTRING: u32 = fourcc(b"lnam");
pub(super) const DEVICE_PROPERTY_MUTE: u32 = fourcc(b"mute");
pub(super) const OBJECT_PROPERTY_NAME: u32 = fourcc(b"lnam");
pub(super) const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
