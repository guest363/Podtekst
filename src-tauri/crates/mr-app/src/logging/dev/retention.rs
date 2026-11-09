use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub(super) const RUNS_MAX_BYTES: u64 = 200 * 1024 * 1024;
pub(super) const RUNS_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
pub(super) const MAX_DEV_LOGS: usize = 20;
pub(super) const MAX_DEV_LOG_BYTES: u64 = 200 * 1024 * 1024;

pub(super) fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(super) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

pub(super) fn append_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = open_private(path)?;
    file.write_all(bytes)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

pub(super) fn retain_logs(dir: &Path, keep: usize, max_bytes: u64) {
    let mut files: Vec<(PathBuf, SystemTime, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let meta = entry.metadata().ok()?;
            Some((path, meta.modified().ok()?, meta.len()))
        })
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let mut total = 0;
    for (index, (path, _, size)) in files.into_iter().enumerate() {
        total += size;
        if index >= keep || total > max_bytes {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(super) fn retain_runs(root: &Path, keep: usize, max_bytes: u64, max_age: Duration) {
    let mut dirs: Vec<(PathBuf, SystemTime, u64)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let size = dir_size(&path);
            Some((path, meta.modified().ok()?, size))
        })
        .collect();
    dirs.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let now = SystemTime::now();
    let mut total = 0;
    for (index, (path, modified, size)) in dirs.into_iter().enumerate() {
        total += size;
        let old = now
            .duration_since(modified)
            .map(|d| d > max_age)
            .unwrap_or(false);
        if index >= keep || total > max_bytes || old {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
