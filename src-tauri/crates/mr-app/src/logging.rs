//! Файл лога для упакованных сборок: у `.app` и NSIS нет консоли, поэтому
//! `eprintln!` до поддержки не доходит.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
}
