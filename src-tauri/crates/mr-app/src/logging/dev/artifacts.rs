use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::filter::{env_u64, DEFAULT_MAX_MB};
use super::retention::{append_private, write_private};
use super::{current_run_id, runs_dir};

pub(super) static ARTIFACT_BYTES: AtomicU64 = AtomicU64::new(0);
pub(super) static ARTIFACT_TRUNCATED: AtomicBool = AtomicBool::new(false);

pub(super) fn reset_budget() {
    ARTIFACT_BYTES.store(0, Ordering::Relaxed);
    ARTIFACT_TRUNCATED.store(false, Ordering::Relaxed);
}

fn run_dir() -> Option<PathBuf> {
    let id = current_run_id()?;
    Some(runs_dir().join(id))
}

pub(super) fn check_budget(bytes: usize, config_max: u64) -> bool {
    let used = ARTIFACT_BYTES.load(Ordering::Relaxed);
    if used + bytes as u64 > config_max {
        if !ARTIFACT_TRUNCATED.swap(true, Ordering::Relaxed) {
            log::warn!(
                target: "diag",
                limit_mb = config_max / (1024 * 1024),
                used_mb = used / (1024 * 1024);
                "ev=diag.artifact.truncated"
            );
        }
        return false;
    }
    ARTIFACT_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    true
}

pub(crate) fn write_artifact(relative: &str, bytes: &[u8]) -> bool {
    let Some(dir) = run_dir() else {
        return false;
    };
    if !check_budget(
        bytes.len(),
        env_u64("MR_DIAG_MAX_MB", DEFAULT_MAX_MB) * 1024 * 1024,
    ) {
        return false;
    }
    let path = dir.join(relative);
    if let Err(error) = write_private(&path, bytes) {
        log::warn!(
            target: "diag",
            file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            error = error.to_string();
            "ev=diag.artifact.error"
        );
        return false;
    }
    true
}

pub(crate) fn append_artifact(relative: &str, bytes: &[u8]) -> bool {
    let Some(dir) = run_dir() else {
        return false;
    };
    if !check_budget(
        bytes.len(),
        env_u64("MR_DIAG_MAX_MB", DEFAULT_MAX_MB) * 1024 * 1024,
    ) {
        return false;
    }
    let path = dir.join(relative);
    if let Err(error) = append_private(&path, bytes) {
        log::warn!(
            target: "diag",
            file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            error = error.to_string();
            "ev=diag.artifact.error"
        );
        return false;
    }
    true
}

pub(crate) fn write_manifest(value: &serde_json::Value) -> bool {
    let Some(dir) = run_dir() else {
        return false;
    };
    match serde_json::to_vec_pretty(value) {
        Ok(bytes) => {
            let path = dir.join("manifest.json");
            match write_private(&path, &bytes) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!(target: "run", error = error.to_string(); "ev=run.manifest_error");
                    false
                }
            }
        }
        Err(error) => {
            log::warn!(target: "run", error = error.to_string(); "ev=run.manifest_error");
            false
        }
    }
}

pub(crate) fn append_error(value: &serde_json::Value) {
    let Some(dir) = run_dir() else {
        return;
    };
    let Ok(line) = serde_json::to_string(value) else {
        return;
    };
    if let Err(error) = append_private(&dir.join("errors.jsonl"), format!("{line}\n").as_bytes()) {
        log::warn!(target: "run", error = error.to_string(); "ev=run.error_artifact");
    }
}
