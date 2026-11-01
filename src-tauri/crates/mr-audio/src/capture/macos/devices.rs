use std::ffi::{c_void, CStr};
use std::ptr::null;

use super::ffi::*;
use crate::capture::{AudioError, DeviceCatalog, DeviceInfo, Result};

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        selector,
        scope,
        element: OBJECT_ELEMENT_MAIN,
    }
}

fn get_u32(object: u32, selector: u32, scope: u32) -> Option<u32> {
    let addr = address(selector, scope);
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: CoreAudio заполняет ровно u32 по указанному размеру.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            null(),
            &mut size,
            &mut value as *mut u32 as *mut c_void,
        )
    };
    (status == 0 && size as usize == std::mem::size_of::<u32>()).then_some(value)
}

fn get_cf_string(object: u32, selector: u32, scope: u32) -> Option<String> {
    let addr = address(selector, scope);
    let mut value: *const c_void = null();
    let mut size = std::mem::size_of::<*const c_void>() as u32;
    // SAFETY: CoreAudio возвращает CFStringRef; владение остаётся за нами.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            null(),
            &mut size,
            &mut value as *mut *const c_void as *mut c_void,
        )
    };
    if status != 0 || value.is_null() {
        return None;
    }
    let mut buffer = [0i8; 512];
    // SAFETY: буфер нужного размера, строка освобождается после копирования.
    let ok = unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            buffer.len() as isize,
            CF_STRING_ENCODING_UTF8,
        )
    };
    // SAFETY: CFStringRef получен из Get-свойства и больше не используется.
    unsafe { CFRelease(value) };
    if ok == 0 {
        return None;
    }
    // SAFETY: CFStringGetCString завершил буфер нулём.
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn input_channels(device: AudioDeviceId) -> u32 {
    let addr = address(DEVICE_PROPERTY_STREAM_CONFIGURATION, OBJECT_SCOPE_INPUT);
    let mut size = 0u32;
    // SAFETY: запрос размера свойства без данных.
    let status = unsafe { AudioObjectGetPropertyDataSize(device, &addr, 0, null(), &mut size) };
    if status != 0 || size < 8 {
        return 0;
    }
    // AudioBufferList выравнивается по 8 байт: берём Vec<u64>, а не Vec<u8>.
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let mut got = size;
    // SAFETY: буфер достаточно велик и выровнен; CoreAudio пишет не больше size.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &addr,
            0,
            null(),
            &mut got,
            storage.as_mut_ptr() as *mut c_void,
        )
    };
    if status != 0 || got < 8 {
        return 0;
    }
    let bytes = storage.as_ptr() as *const u8;
    // SAFETY: mNumberBuffers — первые 4 байта AudioBufferList, далее идут
    // AudioBuffer с шагом 16 байт (mNumberChannels, mDataByteSize, mData).
    let buffers = unsafe { *(bytes as *const u32) };
    let mut channels = 0u32;
    for i in 0..buffers.min(64) as usize {
        // SAFETY: в пределах выделенного storage; mNumberChannels по смещению 8.
        channels += unsafe { *(bytes.add(8 + i * 16) as *const u32) };
    }
    channels
}

fn device_ids() -> Vec<AudioDeviceId> {
    let addr = address(HARDWARE_PROPERTY_DEVICES, OBJECT_SCOPE_GLOBAL);
    let mut size = 0u32;
    // SAFETY: запрос размера списка устройств.
    let status =
        unsafe { AudioObjectGetPropertyDataSize(OBJECT_SYSTEM, &addr, 0, null(), &mut size) };
    if status != 0 || size < 4 {
        return Vec::new();
    }
    let mut ids = vec![0u32; size as usize / 4];
    let mut got = size;
    // SAFETY: массив u32 нужного размера; CoreAudio пишет AudioDeviceID (u32).
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

fn default_input() -> Option<AudioDeviceId> {
    get_u32(
        OBJECT_SYSTEM,
        HARDWARE_PROPERTY_DEFAULT_INPUT,
        OBJECT_SCOPE_GLOBAL,
    )
}

fn device_name(device: AudioDeviceId) -> String {
    get_cf_string(device, DEVICE_PROPERTY_NAME_CFSTRING, OBJECT_SCOPE_GLOBAL)
        .or_else(|| get_cf_string(device, OBJECT_PROPERTY_NAME, OBJECT_SCOPE_GLOBAL))
        .unwrap_or_else(|| format!("Микрофон {device}"))
}

fn device_muted(device: AudioDeviceId) -> bool {
    get_u32(device, DEVICE_PROPERTY_MUTE, OBJECT_SCOPE_INPUT).is_some_and(|v| v != 0)
}

/// Виртуальный loopback-драйвер (BlackHole/Loopback/Soundflower): для CoreAudio
/// это обычный вход, но по смыслу — системный звук.
fn is_loopback(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "blackhole",
        "soundflower",
        "loopback",
        "virtual cable",
        "vb-cable",
    ]
    .iter()
    .any(|needle| n.contains(needle))
}

pub fn devices() -> DeviceCatalog {
    let mut catalog = DeviceCatalog {
        platform: "darwin".to_string(),
        ..Default::default()
    };
    let ids = device_ids();
    for id in ids.iter().copied() {
        if input_channels(id) == 0 {
            continue;
        }
        let name = device_name(id);
        let system = is_loopback(&name);
        let info = DeviceInfo {
            id: id.to_string(),
            name,
            muted: device_muted(id),
            system,
        };
        if system {
            catalog.system.push(info);
        } else {
            catalog.inputs.push(info);
        }
    }
    catalog.default_input = default_input()
        .filter(|id| catalog.inputs.iter().any(|i| i.id == id.to_string()))
        .map(|id| id.to_string())
        .or_else(|| catalog.inputs.first().map(|i| i.id.clone()));
    catalog.default_system = catalog.system.first().map(|i| i.id.clone());
    catalog
}

/// Адрес микрофона: сохранённый id, иначе системный вход по умолчанию.
pub(super) fn resolve_device(device: Option<&str>) -> Result<AudioDeviceId> {
    if let Some(id) = device.and_then(|s| s.parse::<u32>().ok()) {
        if input_channels(id) > 0 {
            return Ok(id);
        }
    }
    let id = default_input().ok_or_else(|| {
        AudioError::NoDevice(
            "микрофон не найден: проверьте «Системные настройки» → «Звук»".to_string(),
        )
    })?;
    if input_channels(id) == 0 {
        return Err(AudioError::NoDevice(
            "устройство ввода по умолчанию не имеет микрофонных каналов".to_string(),
        ));
    }
    Ok(id)
}

/// Адрес источника системного звука: loopback-устройство, иначе — понятная
/// ошибка с подсказкой про BlackHole.
pub(super) fn resolve_system_device(device: Option<&str>) -> Result<AudioDeviceId> {
    if let Some(id) = device.and_then(|s| s.parse::<u32>().ok()) {
        if input_channels(id) > 0 {
            return Ok(id);
        }
    }
    devices()
        .default_system
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or_else(|| {
            AudioError::NoDevice(
                "системный звук на macOS не найден: установите BlackHole 2ch и \
                 многовыходное устройство (см. README)"
                    .to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::is_loopback;

    #[test]
    fn detects_virtual_loopback_drivers() {
        assert!(is_loopback("BlackHole 2ch"));
        assert!(is_loopback("Loopback Audio"));
        assert!(is_loopback("Soundflower (2ch)"));
        assert!(!is_loopback("MacBook Pro Microphone"));
    }
}
