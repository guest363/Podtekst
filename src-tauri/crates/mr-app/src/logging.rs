//! Файл лога для упакованных сборок: у `.app` и NSIS нет консоли, поэтому
//! `eprintln!` до поддержки не доходит.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// Одна строка лога: тело `POST /api/log` иначе пишет мегабайты в один вызов.
const MAX_LINE_BYTES: usize = 4 * 1024;

static SINK: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static WRITTEN: AtomicU64 = AtomicU64::new(0);
static OWNER: AtomicBool = AtomicBool::new(false);

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
    let file = open_append();
    WRITTEN.store(file_size(&log_path()), Ordering::Relaxed);
    let _ = SINK.set(Mutex::new(file));
    dev::init_if_enabled();
}

fn open_append() -> Option<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // app.log содержит пути записей: на общей машине он только для владельца.
        options.mode(0o600);
    }
    options.open(log_path()).ok()
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Ротация выполняется только процессом-владельцем лога: `--toggle` иначе
/// переименовывает файл, который главный процесс держит открытым.
pub(crate) fn claim_owner() {
    OWNER.store(true, Ordering::Relaxed);
    if WRITTEN.load(Ordering::Relaxed) > MAX_BYTES {
        rotate_now();
    }
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
    if OWNER.load(Ordering::Relaxed) && WRITTEN.load(Ordering::Relaxed) >= MAX_BYTES {
        rotate_now();
    }
    let Some(sink) = SINK.get() else { return };
    let mut text = limit_line(&text);
    if !paths_allowed() {
        text = crate::privacy::redact_text(&text);
    }
    // Poisoned мьютекс не должен навсегда выключать файловый лог: строки
    // нужны именно при разборе сбоя.
    let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = guard.as_mut() {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("{stamp} {text}\n");
        if file.write_all(line.as_bytes()).is_ok() {
            WRITTEN.fetch_add(line.len() as u64, Ordering::Relaxed);
        }
    }
}

/// P12: полный путь — только при явном `MR_LOG_PATHS=full`.
fn paths_allowed() -> bool {
    std::env::var("MR_LOG_PATHS").as_deref() == Ok("full")
}

fn limit_line(text: &str) -> String {
    if text.len() <= MAX_LINE_BYTES {
        return text.to_string();
    }
    let mut cut = MAX_LINE_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut capped = text[..cut].to_string();
    capped.push('…');
    capped
}

fn rotate_now() {
    let Some(sink) = SINK.get() else { return };
    let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
    guard.take();
    rotate(&logs_dir(), MAX_BYTES);
    *guard = open_append();
    WRITTEN.store(file_size(&log_path()), Ordering::Relaxed);
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
    crate::privacy::fnv1a(text)
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
        return path.to_string_lossy().into_owned();
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return "recording".to_string();
    }
    crate::privacy::path_token(&name)
}

/// P12: полный путь — только при явном `MR_LOG_PATHS=full`.
pub(crate) fn file_token(path: &Path) -> String {
    file_token_with(paths_allowed(), path)
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

    #[test]
    fn long_lines_are_capped() {
        let line = "я".repeat(MAX_LINE_BYTES);
        let capped = limit_line(&line);
        assert!(capped.len() <= MAX_LINE_BYTES + 3, "{}", capped.len());
        assert!(capped.ends_with('…'));
        assert_eq!(limit_line("короткая строка"), "короткая строка");
    }
}
