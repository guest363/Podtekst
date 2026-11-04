use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::privacy;

const SCHEMA: u64 = 1;
const MAX_BYTES: u64 = 512 * 1024;
const CURRENT: &str = "recordings.jsonl";
const ROTATED: &str = "recordings.jsonl.1";
const MAX_MESSAGE_CHARS: usize = 1000;

static LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn journal_dir() -> PathBuf {
    crate::config::app_data_dir().join("journal")
}

pub(crate) fn start(session: &str, mode: &str, mic: &str, system: Option<&str>, screen: bool) {
    append_event(
        &journal_dir(),
        json!({
            "schema": SCHEMA,
            "at": now(),
            "event": "recording_start",
            "session": session,
            "mode": mode,
            "mic": privacy::mask_device_name(mic, 0),
            "system": system.map(|name| privacy::mask_device_name(name, 1)),
            "screen": screen,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finish(
    session: &str,
    status: &str,
    duration_sec: f64,
    mode: &str,
    channels: usize,
    file: &Path,
    finalized: bool,
    is_video: bool,
    silent: bool,
    sys_silent: bool,
    warnings: &[String],
) {
    let warnings: Vec<String> = warnings
        .iter()
        .map(|warning| privacy::redact_text(warning))
        .collect();
    let name = file
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let size_bytes = std::fs::metadata(file).map(|meta| meta.len()).unwrap_or(0);
    append_event(
        &journal_dir(),
        json!({
            "schema": SCHEMA,
            "at": now(),
            "event": "recording_finish",
            "session": session,
            "status": status,
            "duration_sec": duration_sec,
            "mode": mode,
            "channels": channels,
            "file": privacy::redact_path(file),
            "file_name": name,
            "size_bytes": size_bytes,
            "file_id": file_id(file),
            "ext": extension(file),
            "finalized": finalized,
            "is_video": is_video,
            "silent": silent,
            "sys_silent": sys_silent,
            "warnings": warnings,
        }),
    );
}

pub(crate) fn error(session: Option<&str>, stage: &str, code: &str, message: &str) {
    append_event(&journal_dir(), error_event(session, stage, code, message));
}

pub(crate) fn transcription_finish(file: &Path, status: &str, duration_sec: f64) {
    append_event(
        &journal_dir(),
        json!({
            "schema": SCHEMA,
            "at": now(),
            "event": "transcription_finish",
            "file": privacy::redact_path(file),
            "file_id": file_id(file),
            "ext": extension(file),
            "status": status,
            "duration_sec": duration_sec,
        }),
    );
}

pub(crate) fn list(limit: usize) -> Vec<Value> {
    list_from(&journal_dir(), limit)
}

pub(crate) fn export_redacted() -> String {
    export_redacted_in(&journal_dir())
}

pub(crate) fn file_id(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn now() -> String {
    chrono::Local::now().to_rfc3339()
}

fn error_event(session: Option<&str>, stage: &str, code: &str, message: &str) -> Value {
    let message: String = message.chars().take(MAX_MESSAGE_CHARS).collect();
    json!({
        "schema": SCHEMA,
        "at": now(),
        "event": "recording_error",
        "session": session,
        "stage": stage,
        "code": code,
        "message": privacy::redact_text(&message),
    })
}

fn append_event(dir: &Path, event: Value) {
    let Ok(mut line) = serde_json::to_string(&event) else {
        return;
    };
    line.push('\n');
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(CURRENT);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size + line.len() as u64 > MAX_BYTES {
        let rotated = dir.join(ROTATED);
        let _ = std::fs::remove_file(&rotated);
        let _ = std::fs::rename(&path, &rotated);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    // O_APPEND и одна запись: параллельные сессии не перемешивают строки.
    let _ = file.write_all(line.as_bytes());
}

fn read_events(dir: &Path) -> Vec<Value> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    [ROTATED, CURRENT]
        .iter()
        .flat_map(|name| read_lines(&dir.join(name)))
        .collect()
}

fn read_lines(path: &Path) -> Vec<Value> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(Value::is_object)
        .collect()
}

pub(crate) fn total() -> usize {
    sessions_from(&journal_dir()).len()
}

fn list_from(dir: &Path, limit: usize) -> Vec<Value> {
    let mut sessions = sessions_from(dir);
    sessions.truncate(limit);
    sessions
}

fn sessions_from(dir: &Path) -> Vec<Value> {
    let mut sessions: Vec<Value> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for event in read_events(dir) {
        let kind = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "transcription_finish" {
            continue;
        }
        let Some(session) = event.get("session").and_then(Value::as_str) else {
            continue;
        };
        let slot = *index.entry(session.to_string()).or_insert_with(|| {
            sessions.push(json!({
                "id": session,
                "started_at": event.get("at").cloned().unwrap_or(Value::Null),
                "status": "interrupted",
                "errors": [],
            }));
            sessions.len() - 1
        });
        let target = &mut sessions[slot];
        match kind {
            "recording_start" => {
                target["started_at"] = event.get("at").cloned().unwrap_or(Value::Null);
                for key in ["mode", "mic", "system", "screen"] {
                    target[key] = event.get(key).cloned().unwrap_or(Value::Null);
                }
            }
            "recording_finish" => {
                for key in [
                    "status",
                    "duration_sec",
                    "mode",
                    "channels",
                    "finalized",
                    "is_video",
                    "silent",
                    "sys_silent",
                    "warnings",
                ] {
                    target[key] = event.get(key).cloned().unwrap_or(Value::Null);
                }
                target["file"] = json!({
                    "id": event.get("file_id").cloned().unwrap_or(Value::Null),
                    "name": event.get("file_name").cloned().unwrap_or(Value::Null),
                    "size_bytes": event.get("size_bytes").cloned().unwrap_or(Value::Null),
                    "ext": event.get("ext").cloned().unwrap_or(Value::Null),
                });
            }
            "recording_error" => {
                let entry = json!({
                    "at": event.get("at").cloned().unwrap_or(Value::Null),
                    "stage": event.get("stage").cloned().unwrap_or(Value::Null),
                    "code": event.get("code").cloned().unwrap_or(Value::Null),
                    "message": event.get("message").cloned().unwrap_or(Value::Null),
                });
                if let Some(errors) = target.get_mut("errors").and_then(Value::as_array_mut) {
                    errors.push(entry);
                }
                if target.get("status").and_then(Value::as_str) == Some("interrupted") {
                    target["status"] = json!("error");
                }
            }
            _ => {}
        }
    }
    sessions.reverse();
    sessions
}

fn export_redacted_in(dir: &Path) -> String {
    let mut out = String::new();
    for mut event in read_events(dir) {
        if let Some(id) = event
            .get("file_id")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            let ext = event.get("ext").and_then(Value::as_str).unwrap_or("");
            let redacted = redacted_file_name(&id, ext);
            event["file"] = json!(redacted);
            if event.get("file_name").is_some() {
                event["file_name"] = json!(redacted);
            }
        }
        for (key, index) in [("mic", 0usize), ("system", 1)] {
            if let Some(name) = event.get(key).and_then(Value::as_str) {
                event[key] = json!(privacy::mask_device_name(name, index));
            }
        }
        if let Ok(line) = serde_json::to_string(&event) {
            out.push_str(&privacy::redact_text(&line));
            out.push('\n');
        }
    }
    out
}

fn redacted_file_name(id: &str, ext: &str) -> String {
    if ext.is_empty() {
        format!("recording-{id}")
    } else {
        format!("recording-{id}.{ext}")
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-journal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn rotates_after_limit() {
        let dir = temp_dir("rotate");
        let pad = "x".repeat(1024);
        for i in 0..700 {
            append_event(
                &dir,
                json!({"event": "recording_error", "session": "s", "message": format!("{i}{pad}")}),
            );
        }
        let current = std::fs::metadata(dir.join(CURRENT)).unwrap().len();
        assert!(current <= MAX_BYTES);
        assert!(dir.join(ROTATED).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_broken_lines() {
        let dir = temp_dir("broken");
        let body = [
            serde_json::to_string(&json!({
                "event": "recording_start",
                "session": "s1",
                "at": "2026-01-01T10:00:00+00:00",
            }))
            .unwrap(),
            "not-json{{{".to_string(),
            serde_json::to_string(&json!({
                "event": "recording_finish",
                "session": "s1",
                "status": "ok",
            }))
            .unwrap(),
        ]
        .join("\n");
        std::fs::write(dir.join(CURRENT), body).unwrap();
        let list = list_from(&dir, 10);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["status"], "ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_without_finish_is_interrupted() {
        let dir = temp_dir("interrupted");
        append_event(
            &dir,
            json!({
                "event": "recording_start",
                "session": "s1",
                "mode": "mix",
                "mic": "Устройство 1",
            }),
        );
        let list = list_from(&dir, 10);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["status"], "interrupted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_without_finish_is_error() {
        let dir = temp_dir("error");
        append_event(&dir, json!({"event": "recording_start", "session": "s1"}));
        append_event(
            &dir,
            error_event(Some("s1"), "capture", "failed", "нет входа"),
        );
        let list = list_from(&dir, 10);
        assert_eq!(list[0]["status"], "error");
        assert_eq!(list[0]["errors"][0]["code"], "failed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_replaces_file_and_masks_devices() {
        let dir = temp_dir("export");
        append_event(
            &dir,
            json!({
                "event": "recording_start",
                "session": "s1",
                "mic": privacy::mask_device_name("AirPods Pro Test", 0),
                "system": privacy::mask_device_name("BlackHole 2ch", 1),
            }),
        );
        let file = Path::new("/home/example/Recordings/take.wav");
        append_event(
            &dir,
            json!({
                "event": "recording_finish",
                "session": "s1",
                "status": "ok",
                "file": privacy::redact_path(file),
                "file_id": file_id(file),
                "ext": "wav",
            }),
        );
        let text = export_redacted_in(&dir);
        assert!(text.contains("Устройство 1"));
        assert!(text.contains("BlackHole 2ch"));
        assert!(text.contains("recording-"));
        assert!(text.contains(".wav"));
        assert!(!text.contains("AirPods Pro Test"));
        assert!(!text.contains("take.wav"));
        assert!(!text.contains("/home/example"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_truncates_but_total_keeps_all() {
        let dir = temp_dir("total");
        for i in 0..25 {
            append_event(
                &dir,
                json!({"event": "recording_start", "session": format!("s{i}")}),
            );
        }
        assert_eq!(sessions_from(&dir).len(), 25);
        assert_eq!(list_from(&dir, 10).len(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_id_is_stable_and_distinct() {
        let a = file_id(Path::new("/tmp/a.wav"));
        assert_eq!(a, file_id(Path::new("/tmp/a.wav")));
        assert_ne!(a, file_id(Path::new("/tmp/b.wav")));
        assert_eq!(a.len(), 16);
    }
}
