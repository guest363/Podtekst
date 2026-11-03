use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const LOG_TAIL_BYTES: usize = 200 * 1024;
const LOG_TAIL_LINES: usize = 500;
const SAFE_CONFIG_KEYS: [&str; 7] = [
    "record_mode",
    "record_mode_explicit",
    "auto_transcribe",
    "ctc_enabled",
    "diar",
    "new_speaker_min_sim",
    "assign_min_sim",
];

pub(crate) fn output_dir() -> PathBuf {
    crate::config::app_data_dir().join("diagnostics")
}

pub(crate) fn collect() -> Result<PathBuf, String> {
    collect_into(&output_dir())
}

pub(crate) fn collect_into(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Не создана папка отчёта: {e}"))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("podtekst-diagnostics-{stamp}.zip"));
    write_zip(&path, &report_files())?;
    Ok(path)
}

fn report_files() -> Vec<(&'static str, String)> {
    let data = crate::config::app_data_dir();
    let system = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "data_dir": redact_path(&data),
    });
    let permissions = json!({
        "system_audio_permission": mr_audio::capture::system_audio_permission(),
        "screen_capture_supported": crate::screen::is_screen_capture_supported(),
        "ffmpeg": crate::ffmpeg_dl::status_json(),
    });
    let models = json!({
        "models": mr_models::get_models_status(&data, &mr_models::load_bundled_manifest()),
    });
    vec![
        ("system.json", pretty(system)),
        ("permissions.json", pretty(permissions)),
        ("devices.json", pretty(devices_snapshot())),
        ("models.json", pretty(models)),
        (
            "config.json",
            pretty(config_snapshot(&crate::config::load_config())),
        ),
        ("logs/app.log", redact_text(&log_tail())),
        ("README.txt", readme().to_string()),
    ]
}

fn devices_snapshot() -> Value {
    let catalog = mr_audio::capture::devices();
    let mask = |list: &[mr_audio::capture::DeviceInfo]| -> Vec<Value> {
        list.iter()
            .enumerate()
            .map(|(i, d)| {
                json!({
                    "name": masked_device_name(&d.name, i),
                    "muted": d.muted,
                    "system": d.system,
                })
            })
            .collect()
    };
    json!({
        "platform": catalog.platform,
        "inputs": mask(&catalog.inputs),
        "system": mask(&catalog.system),
        "has_default_input": catalog.default_input.is_some(),
        "has_default_system": catalog.default_system.is_some(),
    })
}

fn config_snapshot(cfg: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in SAFE_CONFIG_KEYS {
        if let Some(value) = cfg.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(out)
}

fn log_tail() -> String {
    let path = crate::logging::log_path();
    let bytes = std::fs::read(&path)
        .or_else(|_| std::fs::read(path.with_extension("log.1")))
        .unwrap_or_default();
    let start = bytes.len().saturating_sub(LOG_TAIL_BYTES);
    let text = String::from_utf8_lossy(&bytes[start..]);
    let tail: Vec<&str> = text.lines().rev().take(LOG_TAIL_LINES).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn write_zip(path: &Path, files: &[(&str, String)]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("Не создан архив: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in files {
        zip.start_file(*name, options)
            .map_err(|e| format!("Ошибка архива: {e}"))?;
        zip.write_all(body.as_bytes())
            .map_err(|e| format!("Ошибка записи отчёта: {e}"))?;
    }
    zip.finish().map_err(|e| format!("Ошибка архива: {e}"))?;
    Ok(())
}

fn pretty(value: Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

fn redact_path(path: &Path) -> String {
    redact_with(&path.to_string_lossy(), home_dir().as_deref())
}

fn redact_text(text: &str) -> String {
    redact_with(text, home_dir().as_deref())
}

fn redact_with(text: &str, home: Option<&str>) -> String {
    match home {
        Some(home) if !home.is_empty() => text.replace(home, "~"),
        _ => text.to_string(),
    }
}

fn home_dir() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
}

/// Имена устройств могут содержать имя владельца: оставляем только тип,
/// а виртуальные драйверы и SCK сохраняем - они нужны для диагностики.
fn masked_device_name(name: &str, index: usize) -> String {
    let lower = name.to_lowercase();
    let known = [
        "blackhole",
        "soundflower",
        "loopback",
        "virtual cable",
        "screencapturekit",
    ];
    if known.iter().any(|needle| lower.contains(needle)) {
        name.to_string()
    } else {
        format!("Устройство {}", index + 1)
    }
}

fn readme() -> &'static str {
    "Отчёт диагностики Подтекст.\n\
     \n\
     Состав: версия и платформа, разрешения, аудиоустройства, состояние\n\
     моделей и ffmpeg, настройки записи, последние строки лога.\n\
     \n\
     В архиве нет аудио, расшифровок, голосовых отпечатков и контактов.\n\
     Домашний каталог в путях заменён на ~, имена устройств скрыты.\n\
     Перед отправкой просмотрите logs/app.log: фрагменты текста могут\n\
     попасть в сообщения об ошибках.\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_home_prefix_only() {
        let text = "/home/user/recordings/2026-11-03.wav";
        assert_eq!(
            redact_with(text, Some("/home/user")),
            "~/recordings/2026-11-03.wav"
        );
        assert_eq!(redact_with(text, None), text);
    }

    #[test]
    fn masks_personal_device_names() {
        assert_eq!(masked_device_name("AirPods Pro Олег", 0), "Устройство 1");
        assert_eq!(masked_device_name("BlackHole 2ch", 1), "BlackHole 2ch");
        assert_eq!(
            masked_device_name("Системный звук (ScreenCaptureKit)", 0),
            "Системный звук (ScreenCaptureKit)"
        );
    }

    #[test]
    fn config_snapshot_drops_personal_keys() {
        let cfg = json!({
            "record_mode": "split",
            "hotwords": ["проект", "фамилия"],
            "save_dir": "/home/user/Downloads",
            "auto_transcribe": true,
        });
        let snapshot = config_snapshot(&cfg);
        assert_eq!(snapshot["record_mode"], "split");
        assert_eq!(snapshot["auto_transcribe"], true);
        assert!(snapshot.get("hotwords").is_none());
        assert!(snapshot.get("save_dir").is_none());
    }
}
