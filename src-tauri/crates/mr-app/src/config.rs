//! Конфигурация приложения: config.json, папка записей, статика UI.

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
    std::fs::read_to_string(app_data_dir().join("config.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

/// Атомарная модификация конфигурации под блокировкой (read-modify-write + atomic rename).
pub(crate) fn update_config<F>(mutate: F)
where
    F: FnOnce(&mut serde_json::Value),
{
    config_lock(|| {
        let mut cfg = load_config();
        mutate(&mut cfg);
        let _ = std::fs::create_dir_all(app_data_dir());
        let p = app_data_dir().join("config.json");
        let tmp = p.with_extension("json.tmp");
        if let Ok(body) = serde_json::to_string_pretty(&cfg) {
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    });
}

/// Режим записи из настроек; используется горячей клавишей и по умолчанию.
pub(crate) fn configured_record_mode() -> String {
    match load_config().get("record_mode").and_then(|x| x.as_str()) {
        Some("mix") => "mix".to_string(),
        _ => "split".to_string(),
    }
}

pub(crate) fn save_record_mode(mode: &str) {
    let m = if mode == "mix" { "mix" } else { "split" };
    update_config(|cfg| cfg["record_mode"] = json!(m));
}

/// Авто-расшифровка после остановки записи. По умолчанию включена.
pub(crate) fn configured_auto_transcribe() -> bool {
    load_config()
        .get("auto_transcribe")
        .and_then(|x| x.as_bool())
        .unwrap_or(true)
}

pub(crate) fn save_auto_transcribe(enabled: bool) {
    update_config(|cfg| cfg["auto_transcribe"] = json!(enabled));
}

pub(crate) fn save_dir() -> PathBuf {
    // 1. config.json -> save_dir, иначе ~/Recordings
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
    let Ok(rd) = std::fs::read_dir(save_dir()) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".part")
            || (name.starts_with('.') && (name.ends_with(".mic.f32") || name.ends_with(".sys.f32")))
        {
            // Защита от удаления активной записи: не трогаем файлы, изменённые < 15 сек назад.
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
