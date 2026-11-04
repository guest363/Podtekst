use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::{fit_bytes, MAX_FILES, MAX_TOTAL_BYTES, SCHEMA};

pub(super) fn write_crash(dir: &Path, record: Value) -> Option<PathBuf> {
    let _ = std::fs::create_dir_all(dir);
    let id = record.get("id")?.as_str()?.to_string();
    let path = dir.join(format!("crash-{id}.json"));
    let bytes = fit_bytes(&record)?;
    std::fs::write(&path, bytes).ok()?;
    prune(dir);
    Some(path)
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = entries
        .flatten()
        .filter(|entry| is_crash_file(&entry.path()))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((entry.path(), meta.len(), meta.modified().ok()?))
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.2));
    let mut total = 0u64;
    for (index, (path, size, _)) in files.iter().enumerate() {
        total += size;
        if index >= MAX_FILES || total > MAX_TOTAL_BYTES {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn crash_paths(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_crash_file(path))
        .collect();
    paths.sort();
    paths.reverse();
    paths
}

fn is_crash_file(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    name.starts_with("crash-") && name.ends_with(".json")
}

pub(super) fn list_in(dir: &Path) -> Vec<Value> {
    let acks = read_acks(dir);
    crash_paths(dir)
        .iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            let mut value: Value = serde_json::from_str(&text).ok()?;
            if !value.is_object() {
                return None;
            }
            let acked = value
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| acks.iter().any(|ack| ack == id));
            value["acked"] = json!(acked);
            Some(value)
        })
        .collect()
}

pub(super) fn ack_in(dir: &Path, id: &str) {
    let mut ids = read_acks(dir);
    if !ids.iter().any(|ack| ack == id) {
        ids.push(id.to_string());
        write_acks(dir, &ids);
    }
}

pub(super) fn ack_all_in(dir: &Path) {
    let ids: Vec<String> = list_in(dir)
        .iter()
        .filter_map(|value| value.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    write_acks(dir, &ids);
}

fn read_acks(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("ack.json")) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_acks(dir: &Path, ids: &[String]) {
    let _ = std::fs::create_dir_all(dir);
    let _ = write_json_atomic(&dir.join("ack.json"), &json!(ids));
}

pub(super) fn run_path(dir: &Path) -> PathBuf {
    dir.join("run.json")
}

pub(super) fn read_run(dir: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(run_path(dir)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.is_object().then_some(value)
}

pub(super) fn crash_after(dir: &Path, started_at: u64) -> bool {
    let threshold = UNIX_EPOCH + Duration::from_secs(started_at);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        is_crash_file(&entry.path())
            && entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .is_some_and(|modified| modified >= threshold)
    })
}

pub(super) fn write_run(dir: &Path, clean: bool) {
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let body = json!({
        "schema": SCHEMA,
        "started_at": started_at,
        "pid": std::process::id(),
        "clean": clean,
        "version": env!("CARGO_PKG_VERSION"),
    });
    let _ = std::fs::create_dir_all(dir);
    let _ = write_json_atomic(&run_path(dir), &body);
}

pub(super) fn write_json_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash::build_record;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn record(id: &str) -> Value {
        build_record(
            id,
            "panic",
            false,
            json!({"message": "boom", "location": "src/main.rs:1:1"}),
            Vec::new(),
            json!({"started_at": 1, "pid": 1, "unclean": true}),
        )
    }

    #[test]
    fn keeps_only_ten_files() {
        let dir = temp_dir("prune");
        for i in 0..12 {
            let id = format!("20260101-1200{i:02}-{i}");
            write_crash(&dir, record(&id));
        }
        assert_eq!(list_in(&dir).len(), MAX_FILES);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_total_size_under_limit() {
        let dir = temp_dir("quota");
        for i in 0..9 {
            let id = format!("20260101-1400{i:02}-{i}");
            let mut value = record(&id);
            value["log_tail"] = json!("x".repeat(250_000));
            write_crash(&dir, value);
        }
        let total: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| is_crash_file(&entry.path()))
            .filter_map(|entry| entry.metadata().ok().map(|meta| meta.len()))
            .sum();
        assert!(total <= MAX_TOTAL_BYTES);
        assert!(list_in(&dir).len() < 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ack_marks_listed_crashes() {
        let dir = temp_dir("ack");
        let id = "20260101-130000-1";
        write_crash(&dir, record(id));
        assert_eq!(list_in(&dir)[0]["acked"], json!(false));
        ack_in(&dir, id);
        assert_eq!(list_in(&dir)[0]["acked"], json!(true));
        ack_all_in(&dir);
        assert_eq!(list_in(&dir)[0]["acked"], json!(true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
