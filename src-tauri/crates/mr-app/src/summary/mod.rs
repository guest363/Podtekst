#![cfg_attr(target_os = "android", allow(dead_code))]

mod backend;
mod cache;
pub mod catalog;
pub mod config;
mod fake;
#[cfg(not(target_os = "android"))]
pub mod gpu;
#[cfg(target_os = "android")]
pub mod gpu {
    use serde_json::{json, Value};

    use super::ApiError;

    pub fn status_json() -> Value {
        json!({
            "supported": false,
            "installed": false,
            "installing": false,
            "reason": "unsupported",
        })
    }

    pub fn install() -> Result<(), ApiError> {
        Err(ApiError::new(
            501,
            "gpu_unsupported",
            "Ускорение GPU на этой платформе не поддерживается.",
        ))
    }

    pub fn cancel() -> bool {
        false
    }

    pub fn remove() -> Result<Value, ApiError> {
        Err(ApiError::new(
            501,
            "gpu_unsupported",
            "Ускорение GPU на этой платформе не поддерживается.",
        ))
    }
}
mod id;
mod job;
mod memory;
mod pipeline;
#[cfg(not(target_os = "android"))]
mod process;
mod runner;
#[cfg(not(target_os = "android"))]
mod runtime_locator;
#[cfg(not(target_os = "android"))]
mod sidecar;
mod store;
mod view;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use serde_json::{json, Value};

pub use catalog::{
    model_path, model_ready, model_status, presets_json, settings_view, update_settings,
};
pub use gpu::{
    cancel as gpu_cancel, install as gpu_install, remove as gpu_remove, status_json as gpu_status,
};
pub use job::{
    active_json, cancel, cancel_for_recording, cancel_for_transcription, generate, is_active,
    status_fields, GenerateInput, StartOutcome,
};
pub use runner::run_raw_prompt;
pub use view::{classify_view, get_view};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, code: &'static str, message: &str) -> Self {
        Self {
            status,
            code,
            message: message.to_string(),
        }
    }

    fn store(error: store::StoreError) -> Self {
        let message = match &error {
            store::StoreError::NoFile => "Не указан файл.",
            store::StoreError::Outside => "Файл вне папки записей.",
            store::StoreError::NoTranscript => "Расшифровка не найдена.",
            store::StoreError::NoText => "В расшифровке нет текста.",
            store::StoreError::TooShort => "В записи мало текста для итогов.",
            store::StoreError::OutputCapped => "Итоги слишком большие для сохранения.",
            store::StoreError::Io(_) => "Внутренняя ошибка итогов.",
        };
        Self::new(error.status(), error.code(), message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryJobError {
    Cancelled,
    Timeout,
    ModelLoad,
    Oom,
    InvalidOutput,
    OutputThin,
    OutputCapped,
    BackendCrashed,
    StaleSource,
    SchemaUnsupported,
    NoTranscript,
    NoText,
    TooShort,
    InvalidPreset,
    Internal(String),
}

impl SummaryJobError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::ModelLoad => "model_load",
            Self::Oom => "oom",
            Self::InvalidOutput => "invalid_output",
            Self::OutputThin => "output_thin",
            Self::OutputCapped => "output_capped",
            Self::BackendCrashed => "backend_crashed",
            Self::StaleSource => "stale_source",
            Self::SchemaUnsupported => "schema_unsupported",
            Self::NoTranscript => "no_transcript",
            Self::NoText => "no_text",
            Self::TooShort => "too_short",
            Self::InvalidPreset => "invalid_preset",
            Self::Internal(_) => "internal",
        }
    }

    fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::ModelLoad | Self::Oom | Self::BackendCrashed | Self::Internal(_)
        )
    }
}

static MODEL_GEN: AtomicU64 = AtomicU64::new(0);
static INIT: OnceLock<()> = OnceLock::new();

pub fn init() {
    INIT.get_or_init(|| {
        #[cfg(not(target_os = "android"))]
        {
            process::cleanup_orphans();
            sidecar::arm_idle_monitor();
        }
    });
}

pub fn shutdown() {
    #[cfg(not(target_os = "android"))]
    sidecar::kill_current();
}

pub fn invalidate_model_cache() {
    MODEL_GEN.fetch_add(1, Ordering::SeqCst);
}

pub(super) fn model_generation() -> u64 {
    MODEL_GEN.load(Ordering::SeqCst)
}

pub fn supported() -> bool {
    !cfg!(target_os = "android")
}

pub fn runtime_ready() -> bool {
    if !supported() || backend::fake_mode().is_some() {
        return supported();
    }
    #[cfg(not(target_os = "android"))]
    {
        runtime_locator::locate().is_ok()
    }
    #[cfg(target_os = "android")]
    {
        false
    }
}

pub fn block_reason(recording: bool) -> Option<&'static str> {
    if recording {
        return Some("recording");
    }
    if transcription_pending() {
        return Some("transcribing");
    }
    if is_active() {
        return Some("busy_summary");
    }
    readiness_reason(headless(), runtime_ready(), model_ready())
}

/// Headless (`--web`) сначала требует runtime: без него скачивание модели
/// не приближает генерацию. Desktop сохраняет прежний порядок.
fn readiness_reason(headless: bool, runtime: bool, model: bool) -> Option<&'static str> {
    if headless && !runtime {
        return Some("runtime_missing");
    }
    if !model {
        return Some("model_missing");
    }
    if !runtime {
        return Some("runtime_missing");
    }
    None
}

fn headless() -> bool {
    !cfg!(feature = "tauri-ui")
}

fn transcription_pending() -> bool {
    #[cfg(feature = "onnx")]
    {
        crate::transcribe::has_pending_work()
    }
    #[cfg(not(feature = "onnx"))]
    {
        false
    }
}

pub fn has_summary(parent: &std::path::Path, stem: &str) -> bool {
    store::has_summary(parent, stem)
}

pub fn system_block() -> Value {
    let status = model_status();
    let model_installed = status.as_ref().map(|m| m.is_installed).unwrap_or(false);
    let runtime = runtime_ready();
    json!({
        "supported": supported(),
        "model_installed": model_installed,
        "runtime": if runtime { "sidecar" } else { "missing" },
        "ready": supported() && model_installed && runtime,
        "has_update": status.map(|m| m.has_update).unwrap_or(false),
    })
}

pub fn ensure_enabled() -> Result<(), ApiError> {
    if !supported() {
        return Err(ApiError::new(
            501,
            "summary_unsupported",
            "Итоги на этом устройстве не поддерживаются.",
        ));
    }
    if !config::enabled() {
        return Err(ApiError::new(
            503,
            "summary_disabled",
            "Итоги отключены в этой сборке.",
        ));
    }
    Ok(())
}

#[cfg(feature = "onnx")]
pub(super) fn activity_start() {
    crate::transcribe::events::start_summary_activity();
}

#[cfg(not(feature = "onnx"))]
pub(super) fn activity_start() {}

#[cfg(feature = "onnx")]
pub(super) fn activity_finish() {
    crate::transcribe::events::finish_summary_activity();
}

#[cfg(not(feature = "onnx"))]
pub(super) fn activity_finish() {}

#[cfg(feature = "onnx")]
pub(super) fn emit_event(event: Value) {
    crate::transcribe::events::emit(event);
}

#[cfg(not(feature = "onnx"))]
pub(super) fn emit_event(_event: Value) {}

#[cfg(test)]
mod tests {
    use super::readiness_reason;

    #[test]
    fn headless_leads_with_runtime_not_model() {
        assert_eq!(
            readiness_reason(true, false, false),
            Some("runtime_missing"),
            "headless без обоих компонентов должен вести к runtime"
        );
        assert_eq!(readiness_reason(false, false, false), Some("model_missing"));
        assert_eq!(readiness_reason(true, true, false), Some("model_missing"));
        assert_eq!(
            readiness_reason(false, false, true),
            Some("runtime_missing")
        );
        assert_eq!(readiness_reason(true, true, true), None);
    }
}
