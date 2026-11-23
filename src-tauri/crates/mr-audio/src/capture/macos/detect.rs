use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::null;
use std::sync::OnceLock;

use super::devices::{
    address, device_ids, device_name, get_cf_string, get_u32, input_channels, is_loopback,
};
use super::ffi::*;
use crate::capture::detect::{
    has_matching_render, merge_clients, ActiveAudioClient, DetectionCapability,
};

pub fn detection_capability() -> DetectionCapability {
    static CAP: OnceLock<DetectionCapability> = OnceLock::new();
    *CAP.get_or_init(|| {
        let addr = address(HARDWARE_PROPERTY_PROCESS_OBJECT_LIST, OBJECT_SCOPE_GLOBAL);
        let mut size = 0u32;
        // SAFETY: запрос размера системного свойства CoreAudio без выделения буфера.
        let status =
            unsafe { AudioObjectGetPropertyDataSize(OBJECT_SYSTEM, &addr, 0, null(), &mut size) };
        if status == 0 {
            DetectionCapability::Process
        } else {
            DetectionCapability::DeviceOnly
        }
    })
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
    let mut raw_captures: Vec<(Option<u32>, Option<String>, bool)> = Vec::new();

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
        if running_in {
            raw_captures.push((pid, bundle_id, running_out));
        }
    }

    let clients = raw_captures
        .into_iter()
        .map(|(pid, process, running_out)| {
            let has_active_render = running_out
                || has_matching_render(pid, process.as_deref(), &render_pids, &render_bundles);
            ActiveAudioClient {
                pid,
                process,
                app_name: None,
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
                has_active_render: false,
            }];
        }
    }
    Vec::new()
}

pub fn active_mic_clients() -> Vec<ActiveAudioClient> {
    match detection_capability() {
        DetectionCapability::Process => active_process_clients(),
        DetectionCapability::DeviceOnly => active_device_fallback(),
        DetectionCapability::Unsupported => Vec::new(),
    }
}
