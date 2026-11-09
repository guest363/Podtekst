use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::privacy::redact_text;

#[cfg(target_os = "android")]
type ShareHandler = Box<dyn Fn(&Path) + Send + Sync + 'static>;

#[cfg(target_os = "android")]
static SHARE_HANDLER: std::sync::OnceLock<std::sync::Mutex<Option<ShareHandler>>> =
    std::sync::OnceLock::new();

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

/// Android: системное меню «Поделиться» открывает JNI-мост из Tauri-окна.
#[cfg(target_os = "android")]
pub(crate) fn set_share_handler(handler: ShareHandler) {
    let slot = SHARE_HANDLER.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(handler);
    }
}

/// true — отчёт передан обработчику, который откроет меню «Поделиться».
pub(crate) fn share(path: &Path) -> bool {
    #[cfg(target_os = "android")]
    {
        let Some(slot) = SHARE_HANDLER.get() else {
            return false;
        };
        let Ok(guard) = slot.lock() else {
            return false;
        };
        let Some(handler) = guard.as_ref() else {
            return false;
        };
        handler(path);
        true
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = path;
        false
    }
}

pub(crate) fn collect(with_dev_logs: bool) -> Result<PathBuf, String> {
    collect_into(&output_dir(), with_dev_logs)
}

pub(crate) fn collect_into(dir: &Path, with_dev_logs: bool) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Не создана папка отчёта: {e}"))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("podtekst-diagnostics-{stamp}.zip"));
    write_zip(&path, &report_files(with_dev_logs))?;
    Ok(path)
}

fn report_files(with_dev_logs: bool) -> Vec<(&'static str, String)> {
    let report = crate::system::snapshot_for_report();
    let mut files = vec![
        ("system.json", pretty(report.clone())),
        ("permissions.json", pretty(report["permissions"].clone())),
        ("devices.json", pretty(report["devices"].clone())),
        ("models.json", pretty(report["models"].clone())),
        (
            "config.json",
            pretty(config_snapshot(&crate::config::load_config())),
        ),
        ("journal.jsonl", crate::journal::export_redacted()),
        ("logs/app.log", redact_text(&log_tail())),
    ];
    if let Some(crash) = crate::crash::latest() {
        files.push(("crashes/latest.json", redact_text(&pretty(crash))));
    }
    // P12: dev-логи попадают в экспорт только по явному флагу и только
    // хвостом из кольца, уже без имён файлов и полного текста.
    if with_dev_logs && crate::logging::dev_logs_enabled() {
        let tail = crate::logging::recent_lines(200).join("\n");
        files.push(("logs/dev.log", redact_text(&tail)));
    }
    files.push(("README.txt", readme().to_string()));
    files
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

fn readme() -> &'static str {
    "Отчёт диагностики Подтекст.\n\
     \n\
     Состав: версия и платформа, разрешения, аудиоустройства, состояние\n\
     моделей и ffmpeg, настройки записи, журнал сессий записи, последняя\n\
     аварийная запись (если была) и последние строки лога.\n\
     \n\
     В архиве нет аудио, расшифровок, голосовых отпечатков и контактов.\n\
     Домашний каталог в путях заменён на ~, имена устройств скрыты,\n\
     имена файлов в журнале заменены на recording-<id>.\n\
     Dev-логи попадают только по явному --with-dev-logs и только хвостом\n\
     последних 200 строк.\n\
     Перед отправкой просмотрите logs/app.log: фрагменты текста могут\n\
     попасть в сообщения об ошибках.\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
