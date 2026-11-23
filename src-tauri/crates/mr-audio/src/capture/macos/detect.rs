use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::null;
use std::sync::atomic::{AtomicU8, Ordering};

use super::devices::{
    address, device_ids, device_name, get_cf_string, get_u32, input_channels, is_loopback,
};
use super::ffi::*;
use crate::capture::detect::{
    has_matching_render, merge_clients, ActiveAudioClient, CaptureState, DetectionCapability,
};

const CAP_UNKNOWN: u8 = 0;
const CAP_PROCESS: u8 = 1;
const CAP_DEVICE_ONLY: u8 = 2;

pub fn detection_capability() -> DetectionCapability {
    static CAP: AtomicU8 = AtomicU8::new(CAP_UNKNOWN);
    match CAP.load(Ordering::Relaxed) {
        CAP_PROCESS => return DetectionCapability::Process,
        CAP_DEVICE_ONLY => return DetectionCapability::DeviceOnly,
        _ => {}
    }
    let addr = address(HARDWARE_PROPERTY_PROCESS_OBJECT_LIST, OBJECT_SCOPE_GLOBAL);
    let mut size = 0u32;
    // SAFETY: запрос размера системного свойства CoreAudio без выделения буфера.
    let status =
        unsafe { AudioObjectGetPropertyDataSize(OBJECT_SYSTEM, &addr, 0, null(), &mut size) };
    if status == 0 {
        CAP.store(CAP_PROCESS, Ordering::Relaxed);
        DetectionCapability::Process
    } else if status == HARDWARE_UNKNOWN_PROPERTY_ERROR {
        CAP.store(CAP_DEVICE_ONLY, Ordering::Relaxed);
        DetectionCapability::DeviceOnly
    } else {
        // Временный сбой coreaudiod не кэшируем, чтобы повторить зонд на следующем тике.
        DetectionCapability::DeviceOnly
    }
}

fn process_object_ids() -> Vec<AudioObjectId> {
    let addr = address(HARDWARE_PROPERTY_PROCESS_OBJECT_LIST, OBJECT_SCOPE_GLOBAL);
    let mut size = 0u32;
    // SAFETY: запрос размера списка процессных объектов CoreAudio.
    let status =
        unsafe { AudioObjectGetPropertyDataSize(OBJECT_SYSTEM, &addr, 0, null(), &mut size) };
    if status != 0 || size < 4 {
        return Vec::new();
    }
    let mut ids = vec![0u32; size as usize / 4];
    let mut got = size;
    // SAFETY: массив u32 выровнен и вмещает не больше size байт.
    let status = unsafe {
        AudioObjectGetPropertyData(
            OBJECT_SYSTEM,
            &addr,
            0,
            null(),
            &mut got,
            ids.as_mut_ptr() as *mut c_void,
        )
    };
    if status != 0 {
        return Vec::new();
    }
    ids.truncate(got as usize / 4);
    ids
}

fn active_process_clients() -> Vec<ActiveAudioClient> {
    let self_pid = std::process::id();
    let mut render_pids = HashSet::new();
    let mut render_bundles = HashSet::new();
    let mut raw_clients: Vec<(Option<u32>, Option<String>, CaptureState, bool)> = Vec::new();

    for obj in process_object_ids() {
        let running_in = get_u32(obj, PROCESS_PROPERTY_IS_RUNNING_INPUT, OBJECT_SCOPE_GLOBAL)
            .is_some_and(|v| v != 0);
        let running_out = get_u32(obj, PROCESS_PROPERTY_IS_RUNNING_OUTPUT, OBJECT_SCOPE_GLOBAL)
            .is_some_and(|v| v != 0);
        if !running_in && !running_out {
            continue;
        }
        let pid = get_u32(obj, PROCESS_PROPERTY_PID, OBJECT_SCOPE_GLOBAL)
            .map(|v| v as i32)
            .filter(|&p| p > 0)
            .map(|p| p as u32);
        if pid == Some(self_pid) {
            continue;
        }
        let bundle_id = get_cf_string(obj, PROCESS_PROPERTY_BUNDLE_ID, OBJECT_SCOPE_GLOBAL)
            .filter(|s| !s.is_empty());

        if running_out {
            if let Some(p) = pid {
                render_pids.insert(p);
            }
            if let Some(b) = bundle_id.as_deref() {
                render_bundles.insert(b.to_ascii_lowercase());
            }
        }
        let capture = if running_in {
            CaptureState::Active
        } else {
            CaptureState::None
        };
        raw_clients.push((pid, bundle_id, capture, running_out));
    }

    let clients = raw_clients
        .into_iter()
        .map(|(pid, process, capture, running_out)| {
            let has_active_render = running_out
                || has_matching_render(pid, process.as_deref(), &render_pids, &render_bundles);
            ActiveAudioClient {
                pid,
                process,
                app_name: None,
                capture,
                has_active_render,
            }
        })
        .collect();
    merge_clients(clients)
}

fn active_device_fallback() -> Vec<ActiveAudioClient> {
    for id in device_ids() {
        if input_channels(id) == 0 || is_loopback(&device_name(id)) {
            continue;
        }
        // CoreAudio принимает DeviceIsRunningSomewhere только в GLOBAL scope;
        // для комбо-устройств на macOS < 14.2 возможен отклик и от вывода.
        if get_u32(
            id,
            DEVICE_PROPERTY_IS_RUNNING_SOMEWHERE,
            OBJECT_SCOPE_GLOBAL,
        )
        .is_some_and(|v| v != 0)
        {
            return vec![ActiveAudioClient {
                pid: None,
                process: None,
                app_name: None,
                capture: CaptureState::Active,
                has_active_render: false,
            }];
        }
    }
    Vec::new()
}

pub fn active_audio_clients() -> Vec<ActiveAudioClient> {
    match detection_capability() {
        DetectionCapability::Process => active_process_clients(),
        DetectionCapability::DeviceOnly => active_device_fallback(),
        DetectionCapability::Unsupported => Vec::new(),
    }
}
