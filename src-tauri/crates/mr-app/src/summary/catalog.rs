use std::io::Read;
use std::sync::{Mutex, OnceLock};

use mr_summarize::Detail;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

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

struct Verified {
    path: std::path::PathBuf,
    len: u64,
    mtime: std::time::SystemTime,
    generation: u64,
    sha: String,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// SHA-256 файла модели с кэшем по (путь, размер, mtime). 2.3 ГБ на каждый
/// job читать нельзя, но подменённый файл обязан изменить mtime/размер.
pub fn verified_model_sha(path: &std::path::Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<Option<Verified>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let metadata = std::fs::metadata(path).ok()?;
    let len = metadata.len();
    let mtime = metadata.modified().ok()?;
    let generation = super::model_generation();
    {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(item) = guard.as_ref() {
            if item.path == path
                && item.len == len
                && item.mtime == mtime
                && item.generation == generation
            {
                return Some(item.sha.clone());
            }
        }
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buf).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let sha = hex(&hasher.finalize());
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(Verified {
        path: path.to_path_buf(),
        len,
        mtime,
        generation,
        sha: sha.clone(),
    });
    Some(sha)
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
                "sections_detail": p.sections.iter().map(|s| json!({
                    "id": s.id,
                    "title": s.title_ru,
                    "role": s.role.as_str(),
                })).collect::<Vec<_>>(),
                "role": p.role,
                "domain": p.domain,
                "sensitive": p.sensitive,
                "speaker_primary": p.speaker_primary,
                "prompt_version": p.prompt_version,
            })
        })
        .collect();
    json!({
        "catalog_version": mr_summarize::preset::CATALOG_VERSION,
        "presets": list,
        "system_prompt_base": mr_summarize::prompt::SYSTEM_PROMPT,
    })
}

pub fn settings_view() -> Value {
    let settings = config::load();
    let pj = presets_json();
    json!({
        "settings": settings.public_json(),
        "presets": pj["presets"],
        "system_prompt_base": pj["system_prompt_base"],
        "model": model_status_json(),
        "capabilities": capabilities(),
    })
}

pub fn update_settings(body: &Value) -> Result<Value, ApiError> {
    let effective = config::load();
    let mut settings = config::stored();
    if let Some(auto) = body.get("auto_suggest").and_then(Value::as_bool) {
        if effective.classifier == ClassifierMode::ManualOnly && auto {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_sha_is_cached_and_rechecks_changed_file() {
        let dir = std::env::temp_dir().join(format!(
            "mr-model-sha-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.gguf");
        std::fs::write(&path, b"abcdef").unwrap();
        let first = verified_model_sha(&path).expect("sha");
        assert_eq!(
            first,
            hex(&Sha256::digest(b"abcdef")),
            "sha не совпал с файлом"
        );
        assert_eq!(verified_model_sha(&path).as_deref(), Some(first.as_str()));
        std::fs::write(&path, b"abcdef-longer").unwrap();
        let second = verified_model_sha(&path).expect("sha");
        assert_ne!(first, second, "кэш не заметил новый файл");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
