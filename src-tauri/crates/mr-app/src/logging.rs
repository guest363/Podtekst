//! Файл лога для упакованных сборок: у `.app` и NSIS нет консоли, поэтому
//! `eprintln!` до поддержки не доходит.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(debug_assertions)]
mod dev;
#[cfg(not(debug_assertions))]
mod dev {
    pub(crate) fn init_if_enabled() {}
    pub(crate) fn begin_run() -> String {
        String::new()
    }
    pub(crate) fn enter_stage(_: &str) {}
    pub(crate) fn current_run_id() -> Option<String> {
        None
    }
    pub(crate) fn current_stage() -> String {
        String::new()
    }
    pub(crate) fn end_run() {}
    pub(crate) fn recent_lines(_: usize) -> Vec<String> {
        Vec::new()
    }
    pub(crate) fn flush() {}
    pub(crate) fn shutdown() {}
    pub(crate) fn dev_logs_enabled() -> bool {
        false
    }
    pub(crate) fn dump_enabled() -> bool {
        false
    }
    pub(crate) fn dump_full() -> bool {
        false
    }
    pub(crate) fn write_artifact(_: &str, _: &[u8]) -> bool {
        false
    }
    pub(crate) fn append_artifact(_: &str, _: &[u8]) -> bool {
        false
    }
    pub(crate) fn write_manifest(_: &serde_json::Value) -> bool {
        false
    }
    pub(crate) fn append_error(_: &serde_json::Value) {}
}

const MAX_BYTES: u64 = 2 * 1024 * 1024;

static SINK: OnceLock<Mutex<Option<File>>> = OnceLock::new();

pub(crate) fn logs_dir() -> PathBuf {
    crate::config::app_data_dir().join("logs")
}

pub(crate) fn log_path() -> PathBuf {
    logs_dir().join("app.log")
}

pub(crate) fn init() {
    if SINK.get().is_some() {
        dev::init_if_enabled();
        return;
    }
    let dir = logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    rotate(&dir, MAX_BYTES);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .ok();
    let _ = SINK.set(Mutex::new(file));
    dev::init_if_enabled();
}

/// Ротация только при старте: за сессию лог не превышает пары мегабайт.
fn rotate(dir: &Path, max_bytes: u64) {
    let current = dir.join("app.log");
    if std::fs::metadata(&current).map(|m| m.len()).unwrap_or(0) < max_bytes {
        return;
    }
    let _ = std::fs::remove_file(dir.join("app.log.3"));
    for i in (1..3).rev() {
        let _ = std::fs::rename(
            dir.join(format!("app.log.{i}")),
            dir.join(format!("app.log.{}", i + 1)),
        );
    }
    let _ = std::fs::rename(&current, dir.join("app.log.1"));
}

pub(crate) fn write(args: std::fmt::Arguments<'_>) {
    let text = format!("{args}");
    eprintln!("{text}");
    let Some(sink) = SINK.get() else { return };
    if let Ok(mut guard) = sink.lock() {
        if let Some(file) = guard.as_mut() {
            let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(file, "{stamp} {text}");
        }
    }
}

pub(crate) fn begin_run() -> String {
    dev::begin_run()
}

pub(crate) fn enter_stage(stage: &str) {
    dev::enter_stage(stage);
}

pub(crate) fn current_run_id() -> Option<String> {
    dev::current_run_id()
}

pub(crate) fn current_stage() -> String {
    dev::current_stage()
}

pub(crate) fn end_run() {
    dev::end_run();
}

pub(crate) fn recent_lines(count: usize) -> Vec<String> {
    dev::recent_lines(count)
}

pub(crate) fn flush() {
    dev::flush();
}

pub(crate) fn shutdown() {
    dev::shutdown();
}

pub(crate) fn dev_logs_enabled() -> bool {
    dev::dev_logs_enabled()
}

pub(crate) fn dump_enabled() -> bool {
    dev::dump_enabled()
}

pub(crate) fn dump_full() -> bool {
    dev::dump_full()
}

pub(crate) fn write_artifact_text(relative: &str, text: &str) -> bool {
    if !dev::dump_enabled() {
        return false;
    }
    dev::write_artifact(relative, text.as_bytes())
}

pub(crate) fn write_artifact_json(relative: &str, value: &serde_json::Value) -> bool {
    if !dev::dump_enabled() {
        return false;
    }
    match serde_json::to_vec_pretty(value) {
        Ok(bytes) => dev::write_artifact(relative, &bytes),
        Err(error) => {
            log::warn!(target: "diag", error = error.to_string(); "ev=diag.artifact.error");
            false
        }
    }
}

pub(crate) fn write_artifact_f32(relative: &str, samples: &[f32]) -> bool {
    if !dev::dump_enabled() {
        return false;
    }
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    dev::write_artifact(relative, &bytes)
}

pub(crate) fn append_artifact_jsonl(relative: &str, value: &serde_json::Value) -> bool {
    if !dev::dump_enabled() {
        return false;
    }
    match serde_json::to_string(value) {
        Ok(line) => dev::append_artifact(relative, format!("{line}\n").as_bytes()),
        Err(error) => {
            log::warn!(target: "diag", error = error.to_string(); "ev=diag.artifact.error");
            false
        }
    }
}

pub(crate) fn write_manifest(value: &serde_json::Value) -> bool {
    dev::write_manifest(value)
}

pub(crate) fn append_error(value: &serde_json::Value) {
    dev::append_error(value);
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextMode {
    Off,
    Preview,
    Full,
}

fn text_mode() -> TextMode {
    match std::env::var("MR_LOG_TEXT").as_deref() {
        Ok("off") => TextMode::Off,
        Ok("full") => TextMode::Full,
        _ => TextMode::Preview,
    }
}

fn preview_with(mode: TextMode, text: &str) -> String {
    match mode {
        TextMode::Off => String::new(),
        TextMode::Full => text.replace(['\n', '\r', '\t'], " "),
        TextMode::Preview => {
            let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let total = collapsed.chars().count();
            let mut out: String = collapsed.chars().take(80).collect();
            if total > 80 {
                out.push('…');
            }
            out
        }
    }
}

/// P12: полный текст — только при явном `MR_LOG_TEXT=full`.
pub(crate) fn text_preview(text: &str) -> String {
    preview_with(text_mode(), text)
}

pub(crate) fn full_text_allowed() -> bool {
    text_mode() == TextMode::Full
}

fn speaker_token_with(full: bool, name: &str) -> String {
    if full {
        return name.to_string();
    }
    if name.trim().is_empty() {
        return "S-".to_string();
    }
    format!("S{:06x}", fnv1a(name) & 0xff_ffff)
}

/// P12: имена спикеров — только при явном `MR_LOG_TEXT=full`.
pub(crate) fn speaker_token(name: &str) -> String {
    speaker_token_with(full_text_allowed(), name)
}

fn file_token_with(full: bool, path: &Path) -> String {
    if full {
        return crate::privacy::redact_path(path);
    }
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let hash = fnv1a(&name);
    if extension.is_empty() {
        format!("recording-{hash:08x}")
    } else {
        format!("recording-{hash:08x}.{extension}")
    }
}

/// P12: полный путь — только при явном `MR_LOG_PATHS=full`.
pub(crate) fn file_token(path: &Path) -> String {
    file_token_with(std::env::var("MR_LOG_PATHS").as_deref() == Ok("full"), path)
}

#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => { $crate::logging::write(format_args!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rotates_only_over_limit_and_keeps_three() {
        let dir = temp_dir("rotate");
        std::fs::write(dir.join("app.log"), b"fresh").unwrap();
        rotate(&dir, 10);
        assert!(dir.join("app.log").exists());

        std::fs::write(dir.join("app.log"), vec![b'x'; 11]).unwrap();
        std::fs::write(dir.join("app.log.1"), b"old1").unwrap();
        rotate(&dir, 10);
        assert!(!dir.join("app.log").exists());
        assert!(dir.join("app.log.1").exists());
        assert!(dir.join("app.log.2").exists());
        assert_eq!(std::fs::read(dir.join("app.log.2")).unwrap(), b"old1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_collapses_and_truncates() {
        assert_eq!(preview_with(TextMode::Off, "текст"), "");
        assert_eq!(preview_with(TextMode::Full, "a\nb"), "a b");
        let long = "слово ".repeat(40);
        let preview = preview_with(TextMode::Preview, &long);
        assert!(preview.chars().count() <= 81, "{preview}");
        assert!(preview.ends_with('…'));
        assert_eq!(
            preview_with(TextMode::Preview, "  два   слова "),
            "два слова"
        );
    }

    #[test]
    fn speaker_tokens_are_stable_and_hide_names() {
        let hidden = speaker_token_with(false, "Тестовый Голос");
        assert!(hidden.starts_with('S'));
        assert!(!hidden.contains("Тестовый"));
        assert_eq!(hidden, speaker_token_with(false, "Тестовый Голос"));
        assert_ne!(hidden, speaker_token_with(false, "Другой голос"));
        assert_eq!(speaker_token_with(true, "Тестовый Голос"), "Тестовый Голос");
    }

    #[test]
    fn file_tokens_hide_names_but_keep_extension() {
        let token = file_token_with(false, Path::new("/tmp/встреча команды.wav"));
        assert!(token.starts_with("recording-"));
        assert!(token.ends_with(".wav"));
        assert!(!token.contains("встреча"));
        assert_eq!(
            token,
            file_token_with(false, Path::new("/tmp/встреча команды.wav"))
        );
        assert!(file_token_with(true, Path::new("/tmp/recording.wav")).ends_with("recording.wav"));
    }
}
