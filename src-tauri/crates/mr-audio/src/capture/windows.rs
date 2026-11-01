//! WASAPI (shared mode); системный звук — `AUDCLNT_STREAMFLAGS_LOOPBACK`
//! на конечной точке рендера.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioCaptureClient, IAudioClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Variant::VT_LPWSTR;

use super::stream::{LevelMeter, RawWriter, StreamingResampler};
use super::{AudioError, CaptureStream, DeviceCatalog, DeviceInfo, Result, StreamKind};

const SUBTYPE_IEEE_FLOAT: windows::core::GUID =
    windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00AA00389B71);

// WAVE_FORMAT_* из msdn: у windows-rs константы разбросаны по фичам (KernelStreaming
// и Multimedia), ради двух u16 фичи не нужны.
const TAG_EXTENSIBLE: u16 = 0xFFFE;
const TAG_IEEE_FLOAT: u16 = 3;

const RPC_E_CHANGED_MODE: windows::core::HRESULT = windows::core::HRESULT(0x8001_0106_u32 as i32);

/// Каталог устройств: микрофоны (eCapture) и выходы (eRender — они же loopback-источники).
pub fn devices() -> DeviceCatalog {
    let mut catalog = DeviceCatalog {
        platform: "win32".to_string(),
        ..Default::default()
    };
    let com_owned = com_init();
    unsafe {
        if let Ok(en) = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            collect_endpoints(&en, eCapture, &mut catalog.inputs);
            collect_endpoints(&en, eRender, &mut catalog.system);
            if let Ok(d) = en.GetDefaultAudioEndpoint(eCapture, eConsole) {
                catalog.default_input = device_id(&d);
            }
            if let Ok(d) = en.GetDefaultAudioEndpoint(eRender, eConsole) {
                catalog.default_system = device_id(&d);
            }
        }
    }
    if com_owned {
        unsafe { CoUninitialize() };
    }
    catalog
}

/// Ошибка устройства возвращается сразу: пользователь узнаёт о проблеме при
/// старте записи, а не обнаруживает пустую дорожку после остановки.
pub fn start_stream(
    kind: StreamKind,
    device: Option<&str>,
    path: &Path,
    rate: u32,
) -> Result<CaptureStream> {
    let endpoint_id = resolve_endpoint_id(kind, device)?;
    let kind_name = match kind {
        StreamKind::Microphone => "microphone",
        StreamKind::System => "system-loopback",
    };
    eprintln!("[WASAPI] {kind_name}: {endpoint_id} -> {}", path.display());
    CaptureStream::spawn(path.to_path_buf(), move |path, level, stop| {
        let path = path.to_path_buf();
        if let Err(e) = run_capture(kind, &endpoint_id, &path, level, stop, rate) {
            eprintln!("[WASAPI] захват прерван: {e}");
        }
    })
}

fn resolve_endpoint_id(kind: StreamKind, device: Option<&str>) -> Result<String> {
    let com_owned = com_init();
    let res = unsafe {
        let Ok(en) = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return Err(AudioError::Other(
                "COM MMDeviceEnumerator недоступен".into(),
            ));
        };
        open_device(&en, kind, device).and_then(|d| {
            device_id(&d).ok_or_else(|| AudioError::Other("id устройства не прочитан".into()))
        })
    };
    if com_owned {
        unsafe { CoUninitialize() };
    }
    res
}

fn com_init() -> bool {
    // Поток наш собственный, но на всякий случай допускаем уже инициализированный COM.
    matches!(
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) },
        h if h.is_ok() || h == RPC_E_CHANGED_MODE
    )
}

fn open_device(en: &IMMDeviceEnumerator, kind: StreamKind, id: Option<&str>) -> Result<IMMDevice> {
    unsafe {
        let (flow, what) = match kind {
            StreamKind::Microphone => (eCapture, "микрофон"),
            StreamKind::System => (eRender, "устройство вывода"),
        };
        if let Some(id) = id.filter(|s| !s.is_empty()) {
            let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
            if let Ok(d) = en.GetDevice(PCWSTR::from_raw(wide.as_ptr())) {
                return Ok(d);
            }
        }
        en.GetDefaultAudioEndpoint(flow, eConsole)
            .map_err(|e| AudioError::NoDevice(format!("{what} не найден: {e}")))
    }
}

fn run_capture(
    kind: StreamKind,
    endpoint_id: &str,
    path: &Path,
    level: &Arc<LevelMeter>,
    stop: &AtomicBool,
    out_rate: u32,
) -> Result<()> {
    let com_owned = com_init();
    let res = capture_loop(kind, endpoint_id, path, level, stop, out_rate);
    if com_owned {
        unsafe { CoUninitialize() };
    }
    res
}

fn capture_loop(
    kind: StreamKind,
    endpoint_id: &str,
    path: &Path,
    level: &Arc<LevelMeter>,
    stop: &AtomicBool,
    out_rate: u32,
) -> Result<()> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| AudioError::Other(format!("COM: {e}")))?;
        let device = open_device(&en, kind, Some(endpoint_id))?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| AudioError::Record(format!("WASAPI Activate: {e}")))?;
        let pwfx = client
            .GetMixFormat()
            .map_err(|e| AudioError::Record(format!("WASAPI GetMixFormat: {e}")))?;
        let (dev_rate, channels, is_float) = parse_format(pwfx);
        let flags = match kind {
            StreamKind::System => AUDCLNT_STREAMFLAGS_LOOPBACK,
            StreamKind::Microphone => 0u32,
        };
        if let Err(e) = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 1_000_000, 0, pwfx, None)
        {
            CoTaskMemFree(Some(pwfx.cast()));
            return Err(AudioError::Record(format!("WASAPI Initialize: {e}")));
        }
        CoTaskMemFree(Some(pwfx.cast()));
        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| AudioError::Record(format!("WASAPI GetService: {e}")))?;
        client
            .Start()
            .map_err(|e| AudioError::Record(format!("WASAPI Start: {e}")))?;

        let mut writer = match RawWriter::create(path) {
            Ok(w) => w,
            Err(e) => {
                let _ = client.Stop();
                return Err(AudioError::Other(format!("не создан файл записи: {e}")));
            }
        };
        let mut resampler = StreamingResampler::new(dev_rate, out_rate);
        let mut written: u64 = 0;
        let started = Instant::now();
        let mut io_error: Option<std::io::Error> = None;

        while !stop.load(Ordering::Relaxed) && io_error.is_none() {
            // Все пакеты, накопившиеся за последние 10 мс.
            loop {
                let Ok(pkt) = capture.GetNextPacketSize() else {
                    break;
                };
                if pkt == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let interleaved = read_frames(data, frames, channels, is_float, silent);
                let _ = capture.ReleaseBuffer(frames);
                let mono = downmix(&interleaved, channels);
                let out = match resampler.as_mut() {
                    Some(r) => r.push(&mono),
                    None => mono,
                };
                if !out.is_empty() {
                    level.update(&out);
                    if let Err(e) = writer.write_samples(&out) {
                        io_error = Some(e);
                        break;
                    }
                    written += out.len() as u64;
                }
            }
            // Loopback молчит без воспроизведения — добираем тишину до реального времени.
            if kind == StreamKind::System {
                let expected = (started.elapsed().as_secs_f64() * f64::from(out_rate)) as u64;
                let deficit = expected.saturating_sub(written);
                if deficit > u64::from(out_rate) / 20 {
                    let n = deficit.min(u64::from(out_rate)) as usize;
                    let silence = vec![0.0f32; n];
                    level.update(&silence);
                    if let Err(e) = writer.write_samples(&silence) {
                        io_error = Some(e);
                    } else {
                        written += n as u64;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let _ = client.Stop();
        let _ = writer.flush();
        if let Some(e) = io_error {
            return Err(AudioError::Other(format!("ошибка диска: {e}")));
        }
    }
    Ok(())
}

/// SAFETY: data — буфер из `GetBuffer`, живой до `ReleaseBuffer`; длина ровно
/// frames*channels сэмплов в формате, который вернул `GetMixFormat`.
unsafe fn read_frames(
    data: *mut u8,
    frames: u32,
    channels: u16,
    is_float: bool,
    silent: bool,
) -> Vec<f32> {
    let n = frames as usize * channels as usize;
    if silent || data.is_null() {
        return vec![0.0; n];
    }
    if is_float {
        std::slice::from_raw_parts(data as *const f32, n).to_vec()
    } else {
        std::slice::from_raw_parts(data as *const i16, n)
            .iter()
            .map(|&x| f32::from(x) / 32768.0)
            .collect()
    }
}

fn downmix(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect()
}

unsafe fn parse_format(pwfx: *mut WAVEFORMATEX) -> (u32, u16, bool) {
    let f = &*pwfx;
    // SAFETY: SubFormat читается read_unaligned — WAVEFORMATEXTENSIBLE packed(1).
    let ext = pwfx.cast::<WAVEFORMATEXTENSIBLE>();
    let sub_format = std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
    let is_float = if f.wFormatTag == TAG_EXTENSIBLE {
        sub_format == SUBTYPE_IEEE_FLOAT
    } else {
        f.wFormatTag == TAG_IEEE_FLOAT
    };
    (f.nSamplesPerSec, f.nChannels, is_float)
}

unsafe fn collect_endpoints(en: &IMMDeviceEnumerator, flow: EDataFlow, out: &mut Vec<DeviceInfo>) {
    let Ok(coll) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
        return;
    };
    let Ok(n) = coll.GetCount() else { return };
    for i in 0..n {
        let Ok(dev) = coll.Item(i) else { continue };
        let Some(id) = device_id(&dev) else { continue };
        let name = friendly_name(&dev).unwrap_or_else(|| id.clone());
        out.push(DeviceInfo {
            id,
            name,
            muted: endpoint_muted(&dev),
            system: flow == eRender,
        });
    }
}

unsafe fn device_id(dev: &IMMDevice) -> Option<String> {
    let pw = dev.GetId().ok()?;
    let s = pw.to_string().ok();
    CoTaskMemFree(Some(pw.0.cast()));
    s
}

unsafe fn friendly_name(dev: &IMMDevice) -> Option<String> {
    let store = dev.OpenPropertyStore(STGM_READ).ok()?;
    let pv = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
    propvariant_string(&pv)
}

/// Строка из PROPVARIANT: поддержан только VT_LPWSTR — имена устройств всегда в нём.
unsafe fn propvariant_string(pv: &PROPVARIANT) -> Option<String> {
    let hdr = &pv.Anonymous.Anonymous;
    if hdr.vt != VT_LPWSTR {
        return None;
    }
    let p = hdr.Anonymous.pwszVal;
    if p.0.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *p.0.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(
        p.0, len,
    )))
}

unsafe fn endpoint_muted(dev: &IMMDevice) -> bool {
    dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
        .and_then(|v| v.GetMute())
        .map(|m| m.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Каталог устройств: на CI-раннере звука может не быть — это hardware-тест.
    #[test]
    fn catalog_lists_default_endpoints() {
        let cat = devices();
        assert_eq!(cat.platform, "win32");
        if cat.inputs.is_empty() && cat.system.is_empty() {
            eprintln!("skip: звуковых устройств нет");
            return;
        }
        assert!(
            cat.default_system.is_some() || cat.default_input.is_some(),
            "хотя бы одно устройство по умолчанию"
        );
    }

    /// Живой захват loopback: за 1.5 с дорожка должна набрать реальный хронометраж
    /// (тишина добирается сама — это часть контракта дорожки системного звука).
    #[test]
    fn system_loopback_capture_writes_samples() {
        let cat = devices();
        let Some(dev) = cat.default_system.clone() else {
            eprintln!("skip: устройство вывода не найдено");
            return;
        };
        let dir = std::env::temp_dir().join(format!("mr-wasapi-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sys.f32");
        let mut stream =
            start_stream(StreamKind::System, Some(&dev), &path, 16000).expect("start_stream");
        std::thread::sleep(Duration::from_millis(1500));
        stream.stop();
        let samples = super::super::stream::read_raw_f32(&path).expect("read");
        assert!(
            samples.len() > 16_000,
            "дорожка короче реального времени: {} сэмплов",
            samples.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
