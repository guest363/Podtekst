//! Linux: PipeWire/PulseAudio через `libpulse` — без внешних утилит.
//!
//! Основной путь: прямая работа с системной библиотекой PulseAudio
//! (загружается через `dlopen`, поэтому бинарник собирается без
//! dev-зависимостей и работает в Flatpak). Работает и с обычным
//! PulseAudio, и с его совместимым сервером внутри PipeWire.
//!
//! Резервный путь — утилиты `pw-record`/`parec` (см. `cli.rs`), если
//! системной библиотеки в системе нет.

use std::path::Path;
use std::sync::atomic::Ordering;

use super::cli;
use super::pulse;
use super::stream::{s16le_to_f32, RawWriter};
use super::{AudioError, CaptureStream, DeviceCatalog, DeviceInfo, Result, StreamKind};

/// Каталог устройств: микрофоны и мониторы выходов.
pub fn devices() -> DeviceCatalog {
    let mut catalog = DeviceCatalog {
        platform: "linux".to_string(),
        ..Default::default()
    };
    let Ok(lib) = pulse::lib() else {
        return cli_catalog();
    };
    let Ok(devices) = lib.devices() else {
        return cli_catalog();
    };
    for src in &devices.sources {
        let name = if src.description.is_empty() {
            src.name.clone()
        } else {
            src.description.clone()
        };
        let info = DeviceInfo {
            id: src.name.clone(),
            name,
            muted: src.muted,
            system: src.monitor,
        };
        if src.monitor {
            catalog.system.push(info);
        } else {
            catalog.inputs.push(info);
        }
    }
    // Микрофон по умолчанию: дефолт сервера, если он не монитор.
    if let Some(def) = devices.default_source.as_ref() {
        if catalog.inputs.iter().any(|i| &i.id == def) {
            catalog.default_input = Some(def.clone());
        }
    }
    if catalog.default_input.is_none() {
        catalog.default_input = catalog.inputs.first().map(|i| i.id.clone());
    }
    // Системный звук: монитор выхода по умолчанию.
    if let Some(sink) = devices.default_sink.as_ref() {
        let monitor = format!("{sink}.monitor");
        if catalog.system.iter().any(|s| s.id == monitor) {
            catalog.default_system = Some(monitor);
        }
    }
    if catalog.default_system.is_none() {
        catalog.default_system = catalog.system.first().map(|s| s.id.clone());
    }
    catalog
}

/// Каталог через `pactl` — резервный путь, если нет `libpulse`.
fn cli_catalog() -> DeviceCatalog {
    let mut catalog = DeviceCatalog {
        platform: "linux".to_string(),
        ..Default::default()
    };
    for src in cli::pactl_sources() {
        let monitor = src.ends_with(".monitor");
        let info = DeviceInfo {
            id: src.clone(),
            name: src.rsplit('.').next().unwrap_or(&src).to_string(),
            muted: false,
            system: monitor,
        };
        if monitor {
            catalog.system.push(info);
        } else {
            catalog.inputs.push(info);
        }
    }
    catalog.default_input = catalog.inputs.first().map(|i| i.id.clone());
    catalog.default_system = cli::pactl_default_monitor();
    catalog
}

/// Запустить поток захвата: сначала системная библиотека, затем утилиты.
pub fn start_stream(
    kind: StreamKind,
    device: Option<&str>,
    path: &Path,
    rate: u32,
) -> Result<CaptureStream> {
    if let Ok(lib) = pulse::lib() {
        let stream_name = match kind {
            StreamKind::Microphone => "microphone",
            StreamKind::System => "system-audio",
        };
        match lib.open_capture(device, stream_name, rate) {
            Ok(handle) => return spawn_pulse(lib, handle, path.to_path_buf()),
            Err(e) => eprintln!("libpulse: {e}"),
        }
    }
    // Резервный путь: внешняя утилита (нужно явное имя устройства).
    let device = match device {
        Some(d) => d.to_string(),
        None => match kind {
            StreamKind::Microphone => cli::pactl_sources()
                .into_iter()
                .find(|s| !s.ends_with(".monitor"))
                .ok_or_else(|| AudioError::NoDevice("микрофон не найден".to_string()))?,
            StreamKind::System => cli::pactl_default_monitor()
                .ok_or_else(|| AudioError::NoDevice("монитор выхода не найден".to_string()))?,
        },
    };
    cli::start(&device, path, rate)
}

/// Поток записи через `libpulse`: чтение s16le и запись в общий формат f32.
///
/// Ошибки диска не глотаются молча: первая ошибка сохраняется и репортится
/// в stderr (тихая порча данных пользователя хуже видимой неудачи).
fn spawn_pulse(
    lib: &'static pulse::PulseLib,
    handle: pulse::StreamHandle,
    path: std::path::PathBuf,
) -> Result<CaptureStream> {
    CaptureStream::spawn(path, move |path, level, stop| {
        let mut writer = match RawWriter::create(path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Не удалось создать файл записи: {e}");
                lib.close(handle);
                return;
            }
        };
        let mut io_error: Option<std::io::Error> = None;
        let mut buf = vec![0u8; 1024];
        while !stop.load(Ordering::Relaxed) {
            match lib.read(&handle, &mut buf) {
                Ok(0) => {
                    // Пауза против busy-loop, если сервер отдал пустой блок без блокировки.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(n) => {
                    let samples = s16le_to_f32(&buf[..n]);
                    if samples.is_empty() {
                        continue;
                    }
                    level.update(&samples);
                    if let Err(e) = writer.write_samples(&samples) {
                        io_error.get_or_insert(e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Захват звука прерван: {e}");
                    break;
                }
            }
        }
        lib.close(handle);
        let _ = writer.flush();
        if let Some(e) = io_error {
            eprintln!("Ошибка диска при записи звука: {e}");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_capture() {
        let cat = devices();
        assert!(
            !cat.inputs.is_empty(),
            "должен найтись хотя бы один микрофон"
        );
        assert!(
            cat.default_input.is_some(),
            "должен определиться микрофон по умолчанию"
        );
        let dir = std::env::temp_dir().join(format!("test_cap_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.f32");
        let mut stream = start_stream(
            StreamKind::Microphone,
            cat.default_input.as_deref(),
            &path,
            16000,
        )
        .expect("start_stream failed");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let t_stop = std::time::Instant::now();
        stream.stop();
        assert!(
            t_stop.elapsed() < std::time::Duration::from_millis(300),
            "остановка должна быть быстрой (< 300 мс)"
        );
        let samples = crate::capture::stream::read_raw_f32(&path).expect("read failed");
        assert!(!samples.is_empty(), "должны быть записаны сэмплы");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
