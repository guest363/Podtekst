use log::{Level, LevelFilter};

use super::format::{Format, PathMode};

pub(super) const DEFAULT_RING: usize = 2048;
pub(super) const DEFAULT_KEEP_RUNS: usize = 20;
pub(super) const DEFAULT_MAX_MB: u64 = 64;

#[derive(Clone)]
pub(super) struct Filter {
    pub(super) default: LevelFilter,
    pub(super) targets: Vec<(String, LevelFilter)>,
    pub(super) raw: String,
}

impl Filter {
    pub(super) fn parse(raw: &str) -> Self {
        let mut filter = Self {
            default: LevelFilter::Info,
            targets: Vec::new(),
            raw: raw.to_string(),
        };
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match token.split_once('=') {
                Some((target, level)) => {
                    if let Some(level) = parse_level(level.trim()) {
                        filter.targets.push((target.trim().to_string(), level));
                    }
                }
                None => {
                    if let Some(level) = parse_level(token) {
                        filter.default = level;
                    }
                }
            }
        }
        filter
    }

    pub(super) fn enabled(&self, target: &str, level: Level) -> bool {
        let filter = self
            .targets
            .iter()
            .rev()
            .find(|(name, _)| target == name || target.starts_with(&format!("{name}.")))
            .map(|(_, level)| *level)
            .unwrap_or(self.default);
        filter >= level.to_level_filter()
    }

    pub(super) fn max(&self) -> LevelFilter {
        self.targets
            .iter()
            .map(|(_, level)| *level)
            .chain(std::iter::once(self.default))
            .max()
            .unwrap_or(LevelFilter::Off)
    }
}

pub(super) fn parse_level(raw: &str) -> Option<LevelFilter> {
    match raw.to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(super) struct DevConfig {
    pub(super) format: Format,
    pub(super) paths: PathMode,
    pub(super) ring_cap: usize,
}

impl DevConfig {
    pub(super) fn from_env() -> Self {
        Self {
            format: match std::env::var("MR_LOG_FORMAT").as_deref() {
                Ok("jsonl") => Format::Jsonl,
                _ => Format::Text,
            },
            paths: path_mode(),
            ring_cap: env_usize("MR_DIAG_RING", DEFAULT_RING).clamp(16, 65_536),
        }
    }
}

pub(super) fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub(super) fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn path_mode() -> PathMode {
    match std::env::var("MR_LOG_PATHS").as_deref() {
        Ok("full") => PathMode::Full,
        _ => PathMode::Redacted,
    }
}

pub(super) fn dump_mode() -> Option<bool> {
    // None — выключено, Some(full) — полный дамп, Some(false) — обычный.
    match std::env::var("MR_DIAG_DUMP").as_deref() {
        Ok("0") | Ok("off") => None,
        Ok("full") => Some(true),
        _ => Some(false),
    }
}
