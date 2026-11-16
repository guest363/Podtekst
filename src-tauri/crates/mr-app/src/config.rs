use mr_packs as packs;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Mutex;

pub(crate) fn app_data_dir() -> PathBuf {
    packs::default_app_data_dir()
}

/// Сериализация чтения-изменения-записи config.json: воркер расшифровки
/// пишет статистику параллельно HTTP-настройкам, без лока терялись обновления.
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn config_lock<T>(f: impl FnOnce() -> T) -> T {
    let _g = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

pub(crate) fn load_config() -> serde_json::Value {
    parse_config().ok().flatten().unwrap_or_else(|| json!({}))
}

/// `Ok(None)` — файла нет; `Err(())` — файл есть, но не разбирается.
/// Различие нужно, чтобы не затереть настройки молча (save_dir, хотворды, пороги).
fn parse_config() -> Result<Option<serde_json::Value>, ()> {
    let text = match std::fs::read_to_string(config_path()) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Ok(None),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) if value.is_object() => Ok(Some(value)),
        _ => Err(()),
    }
}

fn config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

/// Битый файл остаётся на месте вместе с копией: пользователь может его починить.
fn backup_corrupt_config() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let path = config_path();
        let backup = path.with_extension("json.bak");
        if !backup.exists() {
            let _ = std::fs::copy(&path, &backup);
        }
        crate::note!(
            "ev=config.corrupt file={} backup={}",
            crate::logging::file_token(&path),
            crate::logging::file_token(&backup)
        );
    });
}

pub(crate) fn update_config<F>(mutate: F)
where
    F: FnOnce(&mut serde_json::Value),
{
    config_lock(|| {
        let mut cfg = match parse_config() {
            Ok(Some(cfg)) => cfg,
            Ok(None) => json!({}),
            Err(()) => {
                // Запись поверх битого файла стирает все настройки: не пишем,
                // пока пользователь не починит config.json.
                backup_corrupt_config();
                return;
            }
        };
        mutate(&mut cfg);
        let _ = std::fs::create_dir_all(app_data_dir());
        if let Ok(body) = serde_json::to_string_pretty(&cfg) {
            let _ = crate::fsutil::write_atomic(&config_path(), body.as_bytes());
        }
    });
}

pub(crate) fn configured_record_mode() -> String {
    match load_config().get("record_mode").and_then(|x| x.as_str()) {
        Some("mix") => "mix".to_string(),
        _ => "split".to_string(),
    }
}

pub(crate) fn record_mode_explicit() -> bool {
    load_config()
        .get("record_mode_explicit")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
}

pub(crate) fn save_record_mode(mode: &str) {
    let m = if mode == "mix" { "mix" } else { "split" };
    update_config(|cfg| {
        cfg["record_mode"] = json!(m);
        cfg["record_mode_explicit"] = json!(true);
    });
}

pub(crate) fn configured_auto_transcribe() -> bool {
    load_config()
        .get("auto_transcribe")
        .and_then(|x| x.as_bool())
        .unwrap_or(true)
}

pub(crate) fn save_auto_transcribe(enabled: bool) {
    update_config(|cfg| cfg["auto_transcribe"] = json!(enabled));
}

pub(crate) fn configured_hotwords() -> Vec<String> {
    let raw: Vec<String> = load_config()
        .get("hotwords")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    mr_ml::hotwords::sanitize_hotwords(&raw)
}

pub(crate) fn configured_ctc_enabled() -> bool {
    load_config()
        .get("ctc_enabled")
        .and_then(|x| x.as_bool())
        .unwrap_or(true)
}

#[cfg(feature = "onnx")]
pub(crate) fn configured_diar_assign_min_sim() -> f64 {
    load_config()
        .get("diar")
        .and_then(|d| d.get("assign_min_sim"))
        .and_then(|x| x.as_f64())
        .unwrap_or(0.20)
}

#[cfg(feature = "onnx")]
pub(crate) fn configured_diar_new_speaker_min_sim() -> f64 {
    load_config()
        .get("diar")
        .and_then(|d| d.get("new_speaker_min_sim"))
        .and_then(|x| x.as_f64())
        .unwrap_or(0.35)
}

pub(crate) fn save_hotwords_cfg(hotwords: Vec<String>, ctc_enabled: Option<bool>) {
    update_config(|cfg| {
        cfg["hotwords"] = json!(mr_ml::hotwords::sanitize_hotwords(&hotwords));
        if let Some(e) = ctc_enabled {
            cfg["ctc_enabled"] = json!(e);
        }
    });
}

pub(crate) fn save_dir() -> PathBuf {
    if let Some(s) = load_config().get("save_dir").and_then(|x| x.as_str()) {
        let p = PathBuf::from(shellexpand(s));
        let _ = std::fs::create_dir_all(&p);
        return p;
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let p = app_data_dir().join("Recordings");
        let _ = std::fs::create_dir_all(&p);
        p
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        // На Windows HOME нет — записи падали бы в CWD; там домашняя папка в USERPROFILE.
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let p = PathBuf::from(home).join("Recordings");
        let _ = std::fs::create_dir_all(&p);
        p
    }
}

pub(crate) fn shellexpand(s: &str) -> String {
    if s.starts_with("~/") || s.starts_with("~\\") {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        if !home.is_empty() {
            let sep = if s.starts_with("~/") { "/" } else { "\\" };
            let tail = &s[2..];
            return format!("{home}{sep}{tail}");
        }
    }
    s.to_string()
}

pub(crate) fn static_dir() -> PathBuf {
    // Ищем ui/web/static от текущей директории вверх (dev) или рядом с бинарником (prod)
    let mut cur = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let cand = cur.join("ui").join("web").join("static");
        if cand.is_dir() {
            return cand;
        }
        if !cur.pop() {
            break;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let cand = parent.join("static");
            if cand.is_dir() {
                return cand;
            }
        }
    }
    PathBuf::from("ui/web/static")
}

pub(crate) fn cleanup_stale_files() {
    let dir = save_dir();
    // Сначала восстанавливаем хвост прерванной записи из сырья, и только
    // после успеха сырьё удаляется внутри recover_interrupted.
    let recovered = crate::recording::recover_interrupted(&dir, std::time::Duration::from_secs(15));
    if recovered > 0 {
        crate::note!("Восстановлено прерванных записей: {recovered}");
    }
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // Только собственные недокачанные загрузки (`.<имя>.<наносекунды>.part`):
        // чужие `*.part` в общей папке не трогаем.
        if name.starts_with('.') && name.ends_with(".part") {
            if let Ok(meta) = e.metadata() {
                if let Ok(mtime) = meta.modified() {
                    if mtime.elapsed().unwrap_or_default() < std::time::Duration::from_secs(15) {
                        continue;
                    }
                }
            }
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn corrupt_config_is_not_overwritten() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("corrupt");
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);
        let path = dir.join("config.json");
        std::fs::write(&path, b"{ broken").unwrap();

        update_config(|cfg| cfg["transcribed_count"] = json!(1));

        assert_eq!(std::fs::read(&path).unwrap(), b"{ broken");
        assert!(dir.join("config.json.bak").exists(), "нет резервной копии");

        // Исправленный файл снова обновляется, прежние ключи сохраняются.
        std::fs::write(&path, br#"{"save_dir":"/tmp/x"}"#).unwrap();
        update_config(|cfg| cfg["record_mode"] = json!("mix"));
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["save_dir"], "/tmp/x");
        assert_eq!(saved["record_mode"], "mix");

        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
