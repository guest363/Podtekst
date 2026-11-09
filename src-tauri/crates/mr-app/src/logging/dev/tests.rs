use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Mutex;
use std::time::Duration;

use log::{Level, LevelFilter};

use super::artifacts::{check_budget, ARTIFACT_BYTES, ARTIFACT_TRUNCATED};
use super::filter::{parse_level, Filter};
use super::format::escape_value;
use super::retention::retain_runs;
use super::sink::{enqueue, push_ring};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mr-devlog-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn filter_parses_default_and_targets() {
    let filter = Filter::parse("info,vad=trace,asr=debug");
    assert!(filter.enabled("run", Level::Info));
    assert!(!filter.enabled("run", Level::Debug));
    assert!(filter.enabled("vad", Level::Trace));
    assert!(filter.enabled("vad.window", Level::Trace));
    assert!(filter.enabled("asr", Level::Debug));
    assert!(!filter.enabled("asr", Level::Trace));
    assert!(!filter.enabled("json", Level::Debug));
    assert_eq!(filter.max(), LevelFilter::Trace);

    let off = Filter::parse("off,vad=debug");
    assert!(!off.enabled("run", Level::Error));
    assert!(off.enabled("vad", Level::Debug));
    assert_eq!(Filter::parse("nonsense").default, LevelFilter::Info);
    assert_eq!(parse_level("TRACE"), Some(LevelFilter::Trace));
    assert_eq!(parse_level("nonsense"), None);
}

#[test]
fn escape_quotes_spaces_newlines_and_unicode() {
    assert_eq!(escape_value("plain"), "plain");
    assert_eq!(escape_value("два слова"), "\"два слова\"");
    assert_eq!(escape_value("a\"b"), "\"a\\\"b\"");
    assert_eq!(escape_value("a\nb"), "\"a\\nb\"");
    assert_eq!(escape_value("путь\\x"), "\"путь\\\\x\"");
}

#[test]
fn ring_keeps_only_last_lines() {
    let ring = Mutex::new(VecDeque::new());
    for i in 0..5 {
        push_ring(&ring, 3, format!("line {i}"));
    }
    let guard = ring.lock().unwrap();
    assert_eq!(guard.len(), 3);
    assert_eq!(guard.front().map(String::as_str), Some("line 2"));
    assert_eq!(guard.back().map(String::as_str), Some("line 4"));
}

#[test]
fn full_queue_increments_dropped_without_blocking() {
    let (sender, _receiver) = sync_channel(1);
    let dropped = AtomicU64::new(0);
    enqueue(&sender, "one".to_string(), &dropped);
    enqueue(&sender, "two".to_string(), &dropped);
    enqueue(&sender, "three".to_string(), &dropped);
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
}

#[test]
fn run_retention_keeps_newest_and_drops_old() {
    let dir = temp_dir("runs");
    for i in 0..5 {
        let run = dir.join(format!("run-{i}"));
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(run.join("data.bin"), vec![0u8; 10]).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    retain_runs(&dir, 2, 1024 * 1024, Duration::from_secs(3600));
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    left.sort();
    assert_eq!(left, vec!["run-3", "run-4"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn artifact_budget_reports_truncation_once() {
    ARTIFACT_BYTES.store(0, Ordering::Relaxed);
    ARTIFACT_TRUNCATED.store(false, Ordering::Relaxed);
    assert!(check_budget(10, 100));
    assert!(check_budget(80, 100));
    assert!(!check_budget(20, 100));
    assert!(ARTIFACT_TRUNCATED.load(Ordering::Relaxed));
    assert_eq!(ARTIFACT_BYTES.load(Ordering::Relaxed), 90);
}

#[test]
fn run_id_is_stable_shape() {
    let id = super::new_run_id();
    let parts: Vec<&str> = id.split('-').collect();
    assert_eq!(parts.len(), 3, "{id}");
    assert_eq!(parts[2].len(), 4);
    assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()));
}
