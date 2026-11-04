mod checks;
mod health;
mod probe;

pub(crate) use probe::cleanup_stale;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::config::app_data_dir;
use crate::privacy::{mask_device_name, redact_text};

pub(crate) fn snapshot(recording: bool, elapsed: u64, mode: &str) -> Value {
    let data = app_data_dir();
    let recordings = crate::config::save_dir();
    let manifest = mr_models::load_bundled_manifest();
    let models = mr_models::get_models_status(&data, &manifest);
    let is_core = |id: &str| {
        let id = id.to_lowercase();
        !id.contains("whisper") && !id.contains("ctc")
    };
    let core: Vec<&mr_models::ModelStatus> = models.iter().filter(|m| is_core(&m.id)).collect();
    let missing_core: Vec<String> = core
        .iter()
        .filter(|m| !m.is_installed)
        .map(|m| m.id.clone())
        .collect();
    let updates: Vec<String> = models
        .iter()
        .filter(|m| m.has_update)
        .map(|m| m.id.clone())
        .collect();
    let cfg = crate::config::load_config();
    let attempt = cfg.get("update_attempt");
    let log = crate::logging::log_path();
    let log_meta = std::fs::metadata(&log).ok();

    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": {
            "name": std::env::consts::OS,
            "family": std::env::consts::FAMILY,
            "arch": std::env::consts::ARCH,
        },
        "paths": {
            "data": data.to_string_lossy(),
            "recordings": recordings.to_string_lossy(),
            "logs": crate::logging::logs_dir().to_string_lossy(),
            "models": data.join("models").to_string_lossy(),
            "crashes": data.join("crashes").to_string_lossy(),
            "diagnostics": crate::diagnostics::output_dir().to_string_lossy(),
        },
        "disk": {
            "total_bytes": fs2::total_space(&recordings).ok(),
            "free_bytes": fs2::available_space(&recordings).ok(),
            "path": recordings.to_string_lossy(),
        },
        "memory": {"available_bytes": crate::sysmem::available_bytes()},
        "recording": {
            "active": recording,
            "elapsed": elapsed,
            "mode": mode,
        },
        "models": {
            "core_ready": !core.is_empty() && missing_core.is_empty(),
            "installed": models.iter().filter(|m| m.is_installed).count(),
            "total": models.len(),
            "missing_core": missing_core,
            "updates": updates,
            "items": models,
        },
        "ffmpeg": crate::ffmpeg_dl::status_json(),
        "devices": devices_value(&mr_audio::capture::devices()),
        "permissions": {
            "system_audio": mr_audio::capture::system_audio_permission(),
            "screen_capture_supported": crate::screen::is_screen_capture_supported(),
            "microphone": "unknown",
        },
        "integration": {
            "hotkey": {
                "accelerator": if cfg!(target_os = "macos") { "Cmd+Shift+R" } else { "Ctrl+Shift+R" },
                "registered": Value::Null,
            },
            "autostart": crate::desktop::is_autostart_enabled(),
            "single_instance": true,
            "ipc": true,
            "updater": update_mode(),
        },
        "update": {
            "current": env!("CARGO_PKG_VERSION"),
            "mode": update_mode(),
            "install_mode": update_mode(),
            "self_update": update_mode() == "self",
            "signed": false,
            "last_check": cfg.get("last_update_check")
                .cloned()
                .or_else(|| attempt.cloned())
                .unwrap_or(Value::Null),
            "last_check_at": cfg.get("last_update_check")
                .and_then(|value| value.get("at"))
                .cloned()
                .unwrap_or(Value::Null),
            "latest": cfg.get("last_update_check")
                .and_then(|value| value.get("latest"))
                .cloned()
                .unwrap_or(Value::Null),
            "skipped_version": cfg.get("skipped_update_version").cloned().unwrap_or(Value::Null),
        },
        "crashes": crash_stats(&data.join("crashes")),
        "journal": dir_stats(&data.join("journal")),
        "log": {
            "path": log.to_string_lossy(),
            "bytes": log_meta.as_ref().map(|m| m.len()),
            "modified_at": log_meta
                .and_then(|m| m.modified().ok())
                .and_then(iso_time),
        },
    })
}

pub(crate) fn snapshot_for_report() -> Value {
    redacted(snapshot(false, 0, &crate::config::configured_record_mode()))
}

/// В отчёте не должно быть домашнего пути и настоящих имён устройств.
pub(crate) fn redacted(mut value: Value) -> Value {
    mask_devices(&mut value);
    redact_value(&mut value);
    value
}

pub(crate) fn check(probe_audio: bool, recording: bool, include_optional: bool) -> Value {
    let snapshot = snapshot(recording, 0, &crate::config::configured_record_mode());
    checks::run(&snapshot, probe_audio, recording, include_optional)
}

pub(crate) fn open_target(target: &str) -> Result<(), String> {
    let dir = match target {
        "recordings" => Some(crate::config::save_dir()),
        "data" => Some(app_data_dir()),
        "logs" => Some(crate::logging::logs_dir()),
        "models" => Some(app_data_dir().join("models")),
        "crashes" => Some(app_data_dir().join("crashes")),
        "diagnostics" => Some(crate::diagnostics::output_dir()),
        _ => None,
    };
    if let Some(dir) = dir {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Не открыта папка {}: {e}", dir.display()))?;
        crate::opener_url(&dir.to_string_lossy());
        return Ok(());
    }
    match target {
        "macos-microphone" => {
            open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        }
        "macos-screen-recording" => open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        ),
        "windows-microphone-privacy" => open_url("ms-settings:privacy-microphone"),
        "windows-sound" => open_url("ms-settings:sound"),
        "linux-sound" => std::process::Command::new("gnome-control-center")
            .arg("sound")
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Не удалось открыть настройки звука: {e}")),
        _ => Err(format!("Неизвестный раздел: {target}")),
    }
}

fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let res = std::process::Command::new("explorer").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let res = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let res = std::process::Command::new("xdg-open").arg(url).spawn();
    res.map(|_| ())
        .map_err(|e| format!("Не удалось открыть системные настройки: {e}"))
}

fn update_mode() -> &'static str {
    if crate::http::update::available_for_status() {
        "self"
    } else {
        "manual"
    }
}

fn devices_value(catalog: &mr_audio::capture::DeviceCatalog) -> Value {
    let list = |items: &[mr_audio::capture::DeviceInfo]| -> Vec<Value> {
        items
            .iter()
            .map(|d| {
                json!({
                    "id": d.id,
                    "name": d.name,
                    "muted": d.muted,
                    "system": d.system,
                })
            })
            .collect()
    };
    json!({
        "platform": catalog.platform,
        "inputs": list(&catalog.inputs),
        "system": list(&catalog.system),
        "default_input": catalog.default_input,
        "default_system": catalog.default_system,
        "has_system_audio": catalog.default_system.is_some() || !catalog.system.is_empty(),
    })
}

fn crash_stats(dir: &Path) -> Value {
    let acked: std::collections::HashSet<String> = std::fs::read_to_string(dir.join("ack.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut count = 0u64;
    let mut last: Option<SystemTime> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("crash-") || !name.ends_with(".json") {
                continue;
            }
            let id = name
                .trim_start_matches("crash-")
                .trim_end_matches(".json")
                .to_string();
            if acked.contains(&id) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            count += 1;
            if let Ok(modified) = meta.modified() {
                last = Some(last.map_or(modified, |prev| prev.max(modified)));
            }
        }
    }
    json!({"count": count, "last_at": last.and_then(iso_time)})
}

fn dir_stats(dir: &Path) -> Value {
    let mut count = 0u64;
    let mut last: Option<SystemTime> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            count += 1;
            if let Ok(modified) = meta.modified() {
                last = Some(last.map_or(modified, |prev| prev.max(modified)));
            }
        }
    }
    json!({"count": count, "last_at": last.and_then(iso_time)})
}

fn iso_time(time: SystemTime) -> Option<String> {
    let secs = time.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn mask_devices(value: &mut Value) {
    let Some(devices) = value.get_mut("devices").and_then(Value::as_object_mut) else {
        return;
    };
    let mut masked_ids = std::collections::HashMap::new();
    for key in ["inputs", "system"] {
        let Some(list) = devices.get_mut(key).and_then(Value::as_array_mut) else {
            continue;
        };
        for (index, item) in list.iter_mut().enumerate() {
            let Some(obj) = item.as_object_mut() else {
                continue;
            };
            let Some(name) = obj.get("name").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            let masked = mask_device_name(&name, index);
            if masked == name {
                continue;
            }
            if let Some(id) = obj.get("id").and_then(Value::as_str) {
                masked_ids.insert(id.to_string(), masked.clone());
            }
            obj.insert("id".into(), json!(masked));
            obj.insert("name".into(), json!(masked));
        }
    }
    for key in ["default_input", "default_system"] {
        let replaced = devices
            .get(key)
            .and_then(Value::as_str)
            .and_then(|id| masked_ids.get(id))
            .cloned();
        if let Some(masked) = replaced {
            devices.insert(key.into(), json!(masked));
        }
    }
}

fn redact_value(value: &mut Value) {
    match value {
        Value::String(text) => *text = redact_text(text),
        Value::Array(items) => {
            for item in items {
                redact_value(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                redact_value(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
fn home_dir() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_target_rejects_unknown_and_traversal() {
        assert!(open_target("../etc/passwd").is_err());
        assert!(open_target("recordings/../../etc/passwd").is_err());
        assert!(open_target("").is_err());
    }

    #[test]
    fn report_redacts_home_and_device_names() {
        let home = match home_dir() {
            Some(home) => home,
            None => return,
        };
        let mut value = json!({
            "paths": {"data": format!("{home}/.local/share/podtekst")},
            "devices": {
                "inputs": [
                    {"id": "alsa:personal", "name": "Наушники Test", "muted": false, "system": false},
                    {"id": "sck", "name": "Системный звук (ScreenCaptureKit)", "muted": false, "system": true},
                ],
                "system": [],
                "default_input": "alsa:personal",
            },
        });
        value = redacted(value);
        let text = value.to_string();
        assert!(!text.contains(&home), "домашний путь не скрыт: {text}");
        assert_eq!(value["devices"]["inputs"][0]["name"], "Устройство 1");
        assert_eq!(value["devices"]["inputs"][0]["id"], "Устройство 1");
        assert_eq!(
            value["devices"]["inputs"][1]["name"],
            "Системный звук (ScreenCaptureKit)"
        );
        assert_eq!(value["devices"]["default_input"], "Устройство 1");
    }

    #[test]
    fn report_snapshot_has_no_home_path() {
        let Some(home) = home_dir() else {
            return;
        };
        let text = snapshot_for_report().to_string();
        assert!(!text.contains(&home), "домашний путь попал в отчёт");
    }

    #[test]
    fn check_without_probe_returns_all_checks() {
        let value = check(false, false, false);
        assert_eq!(value.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            value.get("checks").and_then(Value::as_array).map(Vec::len),
            Some(10)
        );
    }
}
