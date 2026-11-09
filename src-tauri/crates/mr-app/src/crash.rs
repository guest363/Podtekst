mod store;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::privacy;
use store::{
    ack_all_in, ack_in, crash_after, list_in, read_run, write_crash, write_json_atomic, write_run,
};

const SCHEMA: u64 = 1;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_FILES: usize = 10;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024;
const MAX_BACKTRACE_FRAMES: usize = 200;
const MAX_LOG_LINES: usize = 100;
const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 4096;

pub(crate) fn crashes_dir() -> PathBuf {
    crate::config::app_data_dir().join("crashes")
}

/// Пишем панику сами и всегда вызываем прежний hook: консольный вывод
/// остаётся прежним, а диагностика переживает упакованную сборку без консоли.
pub(crate) fn install_hook() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    static IN_HOOK: AtomicBool = AtomicBool::new(false);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Паника внутри самого hook не должна зациклить процесс.
        if !IN_HOOK.swap(true, Ordering::SeqCst) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write_panic(info)));
            IN_HOOK.store(false, Ordering::SeqCst);
        }
        previous(info);
    }));
}

pub(crate) fn begin_run() {
    begin_run_in(&crate::config::app_data_dir());
}

pub(crate) fn mark_clean() {
    mark_clean_in(&crate::config::app_data_dir());
}

pub(crate) fn list() -> Vec<Value> {
    list_in(&crashes_dir())
}

pub(crate) fn latest() -> Option<Value> {
    list().into_iter().next()
}

pub(crate) fn ack(id: &str) {
    ack_in(&crashes_dir(), id);
}

pub(crate) fn ack_all() {
    ack_all_in(&crashes_dir());
}

fn write_panic(info: &std::panic::PanicHookInfo<'_>) {
    let message = if let Some(text) = info.payload().downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = info.payload().downcast_ref::<String>() {
        text.clone()
    } else {
        "panic".to_string()
    };
    let location = info
        .location()
        .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
        .unwrap_or_default();
    let backtrace: Vec<String> = std::backtrace::Backtrace::force_capture()
        .to_string()
        .lines()
        .take(MAX_BACKTRACE_FRAMES)
        .map(privacy::redact_text)
        .collect();
    let dir = crashes_dir();
    let id = new_id(&dir);
    let panic = json!({
        "message": privacy::redact_text(&truncate(&message, MAX_MESSAGE_CHARS)),
        "location": privacy::redact_text(&location),
    });
    let record = build_record(
        &id,
        "panic",
        false,
        panic,
        backtrace,
        run_summary(&crate::config::app_data_dir(), true),
    );
    write_crash(&dir, record);
}

fn build_record(
    id: &str,
    kind: &str,
    synthetic: bool,
    panic: Value,
    backtrace: Vec<String>,
    run: Value,
) -> Value {
    json!({
        "schema": SCHEMA,
        "id": id,
        "created_at": chrono::Local::now().to_rfc3339(),
        "kind": kind,
        "synthetic": synthetic,
        "app": app_info(),
        "os": os_info(),
        "thread": std::thread::current().name().unwrap_or("unnamed"),
        "panic": panic,
        "backtrace": backtrace.into_iter().take(MAX_BACKTRACE_FRAMES).collect::<Vec<_>>(),
        "log_tail": log_tail(),
        "dev_log_tail": dev_log_tail(),
        "run": run,
    })
}

fn dev_log_tail() -> String {
    let lines = crate::logging::recent_lines(200);
    if lines.is_empty() {
        return String::new();
    }
    privacy::redact_text(&lines.join("\n"))
}

fn app_info() -> Value {
    let mut features = Vec::new();
    if cfg!(feature = "onnx") {
        features.push("onnx");
    }
    if cfg!(feature = "tauri-ui") {
        features.push("tauri-ui");
    }
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "features": features,
    })
}

fn os_info() -> Value {
    json!({
        "name": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
    })
}

fn log_tail() -> String {
    let bytes = std::fs::read(crate::logging::log_path()).unwrap_or_default();
    let start = bytes.len().saturating_sub(MAX_LOG_BYTES);
    let text = String::from_utf8_lossy(&bytes[start..]);
    let lines: Vec<&str> = text.lines().rev().take(MAX_LOG_LINES).collect();
    privacy::redact_text(&lines.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn new_id(dir: &Path) -> String {
    let base = format!(
        "{}-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        std::process::id()
    );
    if !dir.join(format!("crash-{base}.json")).exists() {
        return base;
    }
    for suffix in 2..1000 {
        let id = format!("{base}-{suffix}");
        if !dir.join(format!("crash-{id}.json")).exists() {
            return id;
        }
    }
    base
}

fn begin_run_in(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    if let Some(previous) = read_run(dir) {
        let clean = previous
            .get("clean")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let started_at = previous
            .get("started_at")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if !clean && started_at > 0 {
            let crashes = dir.join("crashes");
            if !crash_after(&crashes, started_at) {
                let id = new_id(&crashes);
                let record = build_record(
                    &id,
                    "unclean_exit",
                    true,
                    json!({"message": "", "location": ""}),
                    Vec::new(),
                    json!({
                        "started_at": started_at,
                        "pid": previous.get("pid").cloned().unwrap_or(Value::Null),
                        "unclean": true,
                    }),
                );
                write_crash(&crashes, record);
            }
        }
    }
    write_run(dir, false);
}

fn mark_clean_in(dir: &Path) {
    let Some(mut run) = read_run(dir) else {
        return;
    };
    run["clean"] = json!(true);
    let _ = write_json_atomic(&store::run_path(dir), &run);
}

fn run_summary(dir: &Path, unclean: bool) -> Value {
    match read_run(dir) {
        Some(run) => json!({
            "started_at": run.get("started_at").cloned().unwrap_or(Value::Null),
            "pid": run.get("pid").cloned().unwrap_or(Value::Null),
            "unclean": unclean,
        }),
        None => json!({
            "started_at": Value::Null,
            "pid": std::process::id(),
            "unclean": unclean,
        }),
    }
}

fn fit_bytes(record: &Value) -> Option<Vec<u8>> {
    if let Ok(bytes) = serde_json::to_vec(record) {
        if bytes.len() <= MAX_FILE_BYTES {
            return Some(bytes);
        }
    }
    let mut trimmed = record.clone();
    if let Some(frames) = trimmed.get_mut("backtrace").and_then(Value::as_array_mut) {
        frames.truncate(40);
    }
    let message = trimmed
        .get("panic")
        .and_then(|panic| panic.get("message"))
        .and_then(Value::as_str)
        .map(|text| truncate(text, 1024));
    if let Some(message) = message {
        trimmed["panic"]["message"] = json!(message);
    }
    let tail = trimmed
        .get("log_tail")
        .and_then(Value::as_str)
        .map(|text| truncate(text, 16 * 1024));
    if let Some(tail) = tail {
        trimmed["log_tail"] = json!(tail);
    }
    let dev_tail = trimmed
        .get("dev_log_tail")
        .and_then(Value::as_str)
        .map(|text| truncate(text, 32 * 1024));
    if let Some(tail) = dev_tail {
        trimmed["dev_log_tail"] = json!(tail);
    }
    if let Ok(bytes) = serde_json::to_vec(&trimmed) {
        if bytes.len() <= MAX_FILE_BYTES {
            return Some(bytes);
        }
    }
    trimmed["backtrace"] = json!([]);
    trimmed["log_tail"] = json!("");
    trimmed["dev_log_tail"] = json!("");
    for chars in [4096usize, 1024, 256, 0] {
        let message = trimmed
            .get("panic")
            .and_then(|panic| panic.get("message"))
            .and_then(Value::as_str)
            .map(|text| truncate(text, chars));
        if let Some(message) = message {
            trimmed["panic"]["message"] = json!(message);
        }
        if let Ok(bytes) = serde_json::to_vec(&trimmed) {
            if bytes.len() <= MAX_FILE_BYTES {
                return Some(bytes);
            }
        }
    }
    None
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn record(id: &str, frames: Vec<String>) -> Value {
        build_record(
            id,
            "panic",
            false,
            json!({"message": "boom", "location": "src/main.rs:1:1"}),
            frames,
            json!({"started_at": 1, "pid": 1, "unclean": true}),
        )
    }

    #[test]
    fn crash_file_is_bounded() {
        let dir = temp_dir("bound");
        let frames: Vec<String> = (0..500)
            .map(|i| format!("frame {i} {}", "x".repeat(400)))
            .collect();
        let path = write_crash(&dir, record("20260101-120000-1", frames)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() <= MAX_FILE_BYTES);
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        let count = saved["backtrace"].as_array().unwrap().len();
        assert!(count <= MAX_BACKTRACE_FRAMES);
        assert!(saved["panic"]["message"].as_str().unwrap().len() < 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_record_is_trimmed() {
        let dir = temp_dir("trim");
        let mut value = record("20260101-120000-2", Vec::new());
        value["panic"]["message"] = json!("m".repeat(300_000));
        let bytes = fit_bytes(&value).unwrap();
        assert!(bytes.len() <= MAX_FILE_BYTES);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unclean_run_creates_one_synthetic_crash() {
        let dir = temp_dir("unclean");
        let crashes = dir.join("crashes");
        begin_run_in(&dir);
        mark_clean_in(&dir);
        begin_run_in(&dir);
        assert!(list_in(&crashes).is_empty());

        begin_run_in(&dir);
        let listed = list_in(&crashes);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["kind"], json!("unclean_exit"));
        assert_eq!(listed[0]["synthetic"], json!(true));

        begin_run_in(&dir);
        assert_eq!(list_in(&crashes).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_marker_roundtrip() {
        let dir = temp_dir("marker");
        begin_run_in(&dir);
        assert_eq!(read_run(&dir).unwrap()["clean"], json!(false));
        mark_clean_in(&dir);
        assert_eq!(read_run(&dir).unwrap()["clean"], json!(true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
