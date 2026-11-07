use mr_summarize::classify::{HEURISTIC_VERSION, SPEC_VERSION};
use serde_json::{json, Value};

use super::catalog::{self, capabilities, limits};
use super::config::{self, ClassifierMode};
use super::{active_json, block_reason, cache, ensure_enabled, store, ApiError};

pub fn classify_view(file: &str, force: bool) -> Result<Value, ApiError> {
    ensure_enabled()?;
    if config::load().classifier == ClassifierMode::ManualOnly {
        return Err(ApiError::new(
            409,
            "classifier_manual_only",
            "Рекомендация отключена: недостаточно проверенных данных.",
        ));
    }
    let paths = store::resolve(file).map_err(ApiError::store)?;
    let source = cache::read(&paths).map_err(ApiError::store)?;
    let (classification, cache_hit) = if force {
        (mr_summarize::classify::classify(&source.transcript), false)
    } else {
        cache::classify_cached(&source)
    };
    Ok(json!({
        "classification": classification_json(&classification, &source.rev, cache_hit),
        "source_rev": source.rev,
        "cache_hit": cache_hit,
    }))
}

fn classification_json(
    classification: &mr_summarize::Classification,
    rev: &str,
    cache_hit: bool,
) -> Value {
    json!({
        "preset_id": classification.preset_id,
        "confidence": classification.confidence,
        "alternatives": classification.alternatives,
        "signals": classification.signals,
        "ambiguous": classification.ambiguous,
        "style": classification.style,
        "language": classification.language,
        "source_rev": rev,
        "classified_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "heuristic_version": HEURISTIC_VERSION,
        "spec_version": SPEC_VERSION,
        "user_override": false,
        "cache_hit": cache_hit,
    })
}

pub fn get_view(file: &str, recording: bool) -> Result<Value, ApiError> {
    ensure_enabled()?;
    let paths = store::resolve(file).map_err(ApiError::store)?;
    let (source, source_reason) = match cache::read(&paths) {
        Ok(source) => (Some(source), None),
        Err(store::StoreError::TooShort) => (None, Some("too_short")),
        Err(store::StoreError::NoText) => (None, Some("no_text")),
        Err(error) => return Err(ApiError::store(error)),
    };
    let rev = source.as_ref().map(|s| s.rev.clone()).unwrap_or_default();
    let loaded = if rev.is_empty() {
        store::Loaded {
            state: store::State::Missing,
            artifact: None,
        }
    } else {
        store::load(&paths, &rev)
    };
    let settings = config::load();
    let classification = match (&source, settings.classifier) {
        (Some(source), ClassifierMode::Heuristic) if settings.auto_suggest => {
            let (classification, cache_hit) = cache::classify_cached(source);
            Some(classification_json(&classification, &source.rev, cache_hit))
        }
        _ => None,
    };
    let summary = loaded
        .artifact
        .as_ref()
        .map(|artifact| artifact.project(&paths.relative));
    let mut reason = block_reason(recording);
    if let Some(source_reason) = source_reason {
        reason = Some(source_reason);
    }
    Ok(json!({
        "state": loaded.state.as_str(),
        "source_rev": rev,
        "summary": summary,
        "classification": classification,
        "settings": settings.public_json(),
        "model": catalog::model_status_json(),
        "busy": busy_json(recording),
        "active": active_json(),
        "capabilities": capabilities(),
        "limits": limits(),
        "block_reason": reason,
    }))
}

fn busy_json(recording: bool) -> Value {
    if recording {
        return json!({"kind": "recording"});
    }
    #[cfg(feature = "onnx")]
    if crate::transcribe::is_transcribe_running() {
        return json!({"kind": "transcribing", "file": crate::transcribe::transcribing_file()});
    }
    if let Some(active) = active_json() {
        return json!({
            "kind": "summary",
            "file": active["file"],
            "job_id": active["job_id"],
        });
    }
    Value::Null
}
