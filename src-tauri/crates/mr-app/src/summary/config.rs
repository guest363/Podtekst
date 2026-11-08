use std::path::PathBuf;
#[cfg(test)]
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::SummaryJobError;
use crate::config::app_data_dir;

/// Измеренный пик на 8k контекста (VmHWM 5.424 ГиБ) плюс запас ~1.1 ГиБ.
pub const ESTIMATE_PEAK_MB: u64 = 5530;
pub const REQUIRED_FREE_MB: u64 = 6656;
pub const MIN_SYSTEM_RAM_MB: u64 = 12288;
pub const RECOMMENDED_RAM_MB: u64 = 16384;
pub const MAX_CHARS: usize = 180_000;
pub const MAX_SECONDS: u64 = 7200;
pub const WARN_CHARS: usize = 400;
pub const CONTEXT_TOKENS: u32 = 8192;
pub const MODEL_ID: &str = "sum-qwen3-4b-instruct-2507-q4km";
pub const MODEL_FILE: &str = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
pub const MODEL_TITLE: &str = "Qwen3-4B Instruct 2507 (итоги, Q4_K_M)";
pub const MODEL_TITLE_EN: &str = "Qwen3-4B Instruct 2507 (summary, Q4_K_M)";
pub const MODEL_SHA256: &str = "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597";
pub const RUNTIME_COMMIT: &str = "fd27b2034";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassifierMode {
    Heuristic,
    ManualOnly,
}

impl ClassifierMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heuristic => "heuristic",
            Self::ManualOnly => "manual_only",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "heuristic" => Some(Self::Heuristic),
            "manual_only" => Some(Self::ManualOnly),
            _ => None,
        }
    }
}

/// По умолчанию — эвристика; `manual_only` остаётся операторским откатом
/// после провала на отложенной выборке, а не дефолтом.
fn default_classifier() -> ClassifierMode {
    ClassifierMode::Heuristic
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub auto_suggest: bool,
    #[serde(default)]
    pub default_preset: Option<String>,
    #[serde(default = "default_detail")]
    pub detail: mr_summarize::Detail,
    #[serde(default = "default_classifier")]
    pub classifier: ClassifierMode,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_detail() -> mr_summarize::Detail {
    mr_summarize::Detail::Normal
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_suggest: true,
            default_preset: None,
            detail: default_detail(),
            classifier: default_classifier(),
            enabled: true,
        }
    }
}

impl Settings {
    pub fn public_json(&self) -> serde_json::Value {
        serde_json::json!({
            "auto_suggest": self.auto_suggest,
            "default_preset": self.default_preset,
            "detail": self.detail,
        })
    }
}

#[cfg(test)]
static TEST_CONFIG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn config_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_CONFIG_PATH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        return path.clone();
    }
    app_data_dir().join("summary-config.json")
}

#[cfg(test)]
fn set_test_config_path(path: Option<PathBuf>) {
    *TEST_CONFIG_PATH.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

/// Сохранённые настройки без env-оверрайдов: их и только их пишем в файл, иначе
/// временный `PODTEKST_SUMMARY_ENABLED=0` закрепляется навсегда.
pub(crate) fn stored() -> Settings {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .unwrap_or_default()
}

pub fn load() -> Settings {
    let stored = stored();
    let classifier = std::env::var("SUMMARY_CLASSIFIER_MODE")
        .ok()
        .and_then(|value| ClassifierMode::parse(&value))
        .unwrap_or(stored.classifier);
    let enabled = match std::env::var("PODTEKST_SUMMARY_ENABLED") {
        Ok(value) => value.trim() != "0" && !value.eq_ignore_ascii_case("false"),
        Err(_) => stored.enabled,
    };
    Settings {
        classifier,
        enabled,
        ..stored
    }
}

pub fn save(settings: &Settings) -> Result<(), SummaryJobError> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SummaryJobError::Internal(e.to_string()))?;
    }
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| SummaryJobError::Internal(e.to_string()))?;
    crate::fsutil::write_atomic(&path, text.as_bytes())
        .map_err(|e| SummaryJobError::Internal(e.to_string()))
}

pub fn enabled() -> bool {
    load().enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn default_is_heuristic_not_manual_only() {
        assert_eq!(
            Settings::default().classifier,
            ClassifierMode::Heuristic,
            "D15: manual_only не дефолт"
        );
    }

    #[test]
    fn old_config_without_new_fields_loads() {
        let s: Settings =
            serde_json::from_str(r#"{"auto_suggest":false,"detail":"short"}"#).unwrap();
        assert!(!s.auto_suggest);
        assert_eq!(s.detail, mr_summarize::Detail::Short);
        assert_eq!(s.classifier, ClassifierMode::Heuristic);
        assert!(s.enabled);
        assert!(s.default_preset.is_none());
    }

    #[test]
    fn public_settings_match_dto() {
        let value = Settings::default().public_json();
        assert_eq!(value["detail"], "normal");
        assert_eq!(value["auto_suggest"], true);
        assert!(value.get("classifier").is_none());
    }
    #[test]
    fn env_override_is_not_persisted_by_settings_update() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let old_mode = std::env::var_os("SUMMARY_CLASSIFIER_MODE");
        let old_enabled = std::env::var_os("PODTEKST_SUMMARY_ENABLED");
        let dir = std::env::temp_dir().join(format!(
            "mr-summary-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        set_test_config_path(Some(dir.join("summary-config.json")));
        std::env::set_var("SUMMARY_CLASSIFIER_MODE", "manual_only");
        std::env::set_var("PODTEKST_SUMMARY_ENABLED", "0");
        assert_eq!(load().classifier, ClassifierMode::ManualOnly);
        assert!(!load().enabled);
        super::super::catalog::update_settings(&serde_json::json!({"detail": "short"}))
            .expect("settings update");
        std::env::remove_var("SUMMARY_CLASSIFIER_MODE");
        std::env::remove_var("PODTEKST_SUMMARY_ENABLED");
        let loaded = load();
        assert_eq!(
            loaded.classifier,
            ClassifierMode::Heuristic,
            "env закреплён"
        );
        assert!(loaded.enabled, "env enabled закреплён");
        assert_eq!(loaded.detail, mr_summarize::Detail::Short);
        match old_mode {
            Some(value) => std::env::set_var("SUMMARY_CLASSIFIER_MODE", value),
            None => std::env::remove_var("SUMMARY_CLASSIFIER_MODE"),
        }
        match old_enabled {
            Some(value) => std::env::set_var("PODTEKST_SUMMARY_ENABLED", value),
            None => std::env::remove_var("PODTEKST_SUMMARY_ENABLED"),
        }
        set_test_config_path(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
