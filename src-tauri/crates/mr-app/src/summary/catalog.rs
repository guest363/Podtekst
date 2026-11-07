use mr_summarize::Detail;
use serde_json::{json, Value};

use super::config::{
    self, ClassifierMode, ESTIMATE_PEAK_MB, MAX_CHARS, MAX_SECONDS, MIN_SYSTEM_RAM_MB, MODEL_ID,
    RECOMMENDED_RAM_MB, REQUIRED_FREE_MB, WARN_CHARS,
};
use super::{runtime_ready, ApiError};

pub fn model_status() -> Option<mr_models::ModelStatus> {
    mr_models::get_models_status(
        &crate::config::app_data_dir(),
        &mr_models::load_bundled_manifest(),
    )
    .into_iter()
    .find(|m| m.kind == "summary")
}

pub(super) fn model_status_json() -> Option<Value> {
    let status = model_status()?;
    Some(json!({
        "id": status.id,
        "title": status.title,
        "kind": status.kind,
        "is_installed": status.is_installed,
        "total_size_bytes": status.total_size_bytes,
        "on_disk_bytes": status.on_disk_bytes,
        "ram_mb": status.ram_mb,
        "required_free_mb": status.required_free_mb,
        "context_tokens": status.context_tokens,
        "license": status.license,
        "license_url": status.license_url,
        "source_url": status.source_url,
    }))
}

pub fn model_ready() -> bool {
    model_path().is_some()
}

pub fn model_path() -> Option<std::path::PathBuf> {
    for key in ["MR_SUMMARY_MODEL", "PODTEKST_SUMMARY_MODEL"] {
        if let Ok(value) = std::env::var(key) {
            let path = std::path::PathBuf::from(value);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let path = mr_models::models_dir(&crate::config::app_data_dir())
        .join(MODEL_ID)
        .join(config::MODEL_FILE);
    path.is_file().then_some(path)
}

pub fn capabilities() -> Value {
    json!({
        "summary_supported": super::supported(),
        "runtime": if runtime_ready() { "sidecar" } else { "missing" },
        "classifier": config::load().classifier.as_str(),
    })
}

pub fn limits() -> Value {
    json!({
        "max_chars": MAX_CHARS,
        "max_seconds": MAX_SECONDS,
        "min_free_ram_mb": REQUIRED_FREE_MB,
        "min_system_ram_mb": MIN_SYSTEM_RAM_MB,
        "warn_chars": WARN_CHARS,
        "estimate_peak_mb": ESTIMATE_PEAK_MB,
        "recommended_ram_mb": RECOMMENDED_RAM_MB,
    })
}

pub fn presets_json() -> Value {
    let list: Vec<Value> = mr_summarize::preset::all()
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "label": p.label_ru,
                "short": p.short_ru,
                "caution": p.caution_ru,
                "sections": p.sections.iter().map(|s| s.id).collect::<Vec<_>>(),
                "sensitive": p.sensitive,
            })
        })
        .collect();
    json!({
        "catalog_version": mr_summarize::preset::CATALOG_VERSION,
        "presets": list,
    })
}

pub fn settings_view() -> Value {
    let settings = config::load();
    json!({
        "settings": settings.public_json(),
        "presets": presets_json()["presets"],
        "model": model_status_json(),
        "capabilities": capabilities(),
    })
}

pub fn update_settings(body: &Value) -> Result<Value, ApiError> {
    let mut settings = config::load();
    if let Some(auto) = body.get("auto_suggest").and_then(Value::as_bool) {
        if settings.classifier == ClassifierMode::ManualOnly && auto {
            return Err(ApiError::new(
                409,
                "classifier_manual_only",
                "Рекомендация отключена: недостаточно проверенных данных.",
            ));
        }
        settings.auto_suggest = auto;
    }
    if let Some(preset) = body.get("default_preset") {
        settings.default_preset = match preset.as_str() {
            Some(id) if !id.is_empty() => {
                mr_summarize::preset::get(id)
                    .ok_or_else(|| ApiError::new(400, "invalid_preset", "Неизвестный шаблон."))?;
                Some(id.to_string())
            }
            _ => None,
        };
    }
    if let Some(detail) = body.get("detail").and_then(Value::as_str) {
        settings.detail = Detail::parse(detail)
            .map_err(|_| ApiError::new(400, "invalid_detail", "Неизвестная подробность."))?;
    }
    config::save(&settings).map_err(|_| {
        ApiError::new(
            500,
            "internal",
            "Внутренняя ошибка итогов. Повторите и лог.",
        )
    })?;
    Ok(json!({ "settings": config::load().public_json() }))
}
