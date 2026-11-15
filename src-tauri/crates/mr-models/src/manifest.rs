use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::is_placeholder_sha;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFile {
    pub filename: String,
    pub url_hf: String,
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub optional: bool,
}

/// Пин апстрима: репозиторий и ревизия, с которых сняты файлы модели.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Upstream {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub revision: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub title: String,
    pub version: String,
    #[serde(default = "default_order")]
    pub order: u32,
    #[serde(default)]
    pub upstream: Upstream,
    #[serde(default)]
    pub files: Vec<ModelFile>,
    #[serde(default)]
    pub embedding_space: Option<String>,
    #[serde(default)]
    pub embedding_dim: Option<usize>,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub description: String,
    /// Английское название для англоязычного вывода; пусто — берётся `title`.
    #[serde(default)]
    pub title_en: String,
    /// `asr` для речевых моделей, `summary` для локальных итогов. Старый
    /// манифест без поля читается как `asr`.
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub ram_mb: u64,
    #[serde(default)]
    pub required_free_mb: u64,
    #[serde(default)]
    pub context_tokens: u32,
    #[serde(default)]
    pub min_system_ram_mb: u64,
    #[serde(default)]
    pub license_url: String,
    #[serde(default)]
    pub source_url: String,
}

fn default_kind() -> String {
    "asr".to_string()
}

/// Манифест без `order` (старая сборка) считается первой версией набора.
fn default_order() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsManifest {
    #[serde(default = "default_mv")]
    pub manifest_version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

fn default_mv() -> u32 {
    1
}

static BUNDLED: OnceLock<ModelsManifest> = OnceLock::new();

/// Вшитый манифест: единственный источник истины о версиях, файлах и ревизиях.
pub fn bundled() -> &'static ModelsManifest {
    const BUNDLED_JSON: &str = include_str!("../models-manifest.json");
    BUNDLED.get_or_init(|| {
        serde_json::from_str(BUNDLED_JSON).expect("вшитый models-manifest.json должен разбираться")
    })
}

pub fn load_bundled_manifest() -> ModelsManifest {
    bundled().clone()
}

/// Дата сборки манифеста: UI показывает её как «версии этой сборки».
pub fn manifest_updated_at() -> &'static str {
    &bundled().updated_at
}

/// Битый пин ломает установку у пользователя, поэтому манифест проверяется
/// целиком до первого обращения к сети.
pub fn validate(manifest: &ModelsManifest) -> std::result::Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for m in &manifest.models {
        check_id(&m.id)?;
        if !ids.insert(m.id.as_str()) {
            return Err(format!("id модели повторяется: {}", m.id));
        }
        if m.order == 0 {
            return Err(format!("order модели {} должен быть больше нуля", m.id));
        }
        if m.upstream.kind.is_empty() || m.upstream.repo.is_empty() {
            return Err(format!("у модели {} не указан апстрим", m.id));
        }
        if m.upstream.revision.is_empty() {
            return Err(format!("у модели {} не закреплена ревизия", m.id));
        }
        if m.files.is_empty() {
            return Err(format!("у модели {} нет файлов", m.id));
        }
        let pin = format!("/{}/", m.upstream.revision);
        let mut names = std::collections::HashSet::new();
        for f in &m.files {
            check_filename(&m.id, &f.filename)?;
            if !names.insert(f.filename.as_str()) {
                return Err(format!("файл {} повторяется в модели {}", f.filename, m.id));
            }
            if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!(
                    "sha256 файла {} должен быть 64 hex-символа",
                    f.filename
                ));
            }
            for url in std::iter::once(&f.url_hf).chain(f.mirrors.iter()) {
                check_url(&m.id, url)?;
                if !url.contains(&pin) {
                    return Err(format!(
                        "url файла {} не закреплён на ревизии {}",
                        f.filename, m.upstream.revision
                    ));
                }
            }
        }
    }
    Ok(())
}

fn check_id(id: &str) -> std::result::Result<(), String> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let path = std::path::Path::new(id);
    let stem = id.split('.').next().unwrap_or(id).to_ascii_uppercase();
    let bad = id.is_empty()
        || path.is_absolute()
        || id.contains(['\\', ':', '/'])
        || id.contains("..")
        || id.ends_with(".new")
        || id.contains(".old-")
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || RESERVED.contains(&stem.as_str());
    if bad {
        return Err(format!("недопустимый id {id}"));
    }
    Ok(())
}

fn check_filename(model_id: &str, filename: &str) -> std::result::Result<(), String> {
    let path = std::path::Path::new(filename);
    let bad = filename.is_empty()
        || path.is_absolute()
        || filename.contains('\\')
        || filename.contains(':')
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || filename.ends_with(".part")
        || filename.ends_with(".lock");
    if bad {
        return Err(format!(
            "недопустимое имя файла {filename} в модели {model_id}"
        ));
    }
    Ok(())
}

fn check_url(model_id: &str, url: &str) -> std::result::Result<(), String> {
    if !url.starts_with("https://") || url.contains("REPLACE_") || is_placeholder_sha(url) {
        return Err(format!("недопустимый url {url} в модели {model_id}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> ModelEntry {
        ModelEntry {
            id: "m".to_string(),
            title: "t".to_string(),
            version: "1".to_string(),
            order: 1,
            upstream: Upstream {
                kind: "hf".to_string(),
                repo: "r/x".to_string(),
                revision: "abc".to_string(),
                version: "1".to_string(),
            },
            files: vec![ModelFile {
                filename: "w.onnx".to_string(),
                url_hf: "https://huggingface.co/r/x/resolve/abc/w.onnx".to_string(),
                mirrors: vec![],
                size_bytes: 4,
                sha256: "0".repeat(64),
                optional: false,
            }],
            embedding_space: None,
            embedding_dim: None,
            license: String::new(),
            description: String::new(),
            title_en: String::new(),
            kind: "asr".to_string(),
            tier: String::new(),
            ram_mb: 0,
            required_free_mb: 0,
            context_tokens: 0,
            min_system_ram_mb: 0,
            license_url: String::new(),
            source_url: String::new(),
        }
    }

    fn manifest(models: Vec<ModelEntry>) -> ModelsManifest {
        ModelsManifest {
            manifest_version: 2,
            updated_at: "2026-11-15T00:00:00".to_string(),
            models,
        }
    }

    #[test]
    fn bundled_manifest_is_valid() {
        let bundled = bundled();
        validate(bundled).expect("вшитый манифест должен проходить проверку");
        assert_eq!(bundled.models.len(), 7, "ожидается семь моделей");
        let orders: Vec<u32> = bundled.models.iter().map(|m| m.order).collect();
        assert_eq!(orders, (1..=7).collect::<Vec<u32>>(), "order по порядку");
        assert_eq!(
            bundled
                .models
                .iter()
                .filter(|m| m.kind == "summary")
                .count(),
            1,
            "модель итогов в манифесте ровно одна"
        );
        for m in &bundled.models {
            if let Some(f) = m.files.first() {
                assert!(
                    f.url_hf.contains(&format!("/{}/", m.upstream.revision)),
                    "{} закреплён на main вместо ревизии",
                    m.id
                );
            }
        }
    }

    #[test]
    fn validator_rejects_duplicates_and_placeholders() {
        let mut duplicated = entry();
        duplicated.files.push(duplicated.files[0].clone());
        assert!(
            validate(&manifest(vec![duplicated])).is_err(),
            "дубль файла"
        );

        let mut same_id = entry();
        same_id.version = "1".to_string();
        assert!(
            validate(&manifest(vec![same_id.clone(), same_id])).is_err(),
            "дубль id"
        );

        let mut placeholder = entry();
        placeholder.files[0].sha256 = "REPLACE_ME".to_string();
        assert!(
            validate(&manifest(vec![placeholder])).is_err(),
            "placeholder"
        );

        let mut http = entry();
        http.files[0].url_hf = "http://huggingface.co/r/x/resolve/abc/w.onnx".to_string();
        assert!(validate(&manifest(vec![http])).is_err(), "не HTTPS");

        let mut traversal = entry();
        traversal.files[0].filename = "../w.onnx".to_string();
        assert!(
            validate(&manifest(vec![traversal])).is_err(),
            "выход из каталога"
        );

        let mut absolute = entry();
        absolute.files[0].filename = "/etc/w.onnx".to_string();
        assert!(
            validate(&manifest(vec![absolute])).is_err(),
            "абсолютный путь"
        );

        let mut zero_order = entry();
        zero_order.order = 0;
        assert!(validate(&manifest(vec![zero_order])).is_err(), "order > 0");

        let mut floating = entry();
        floating.files[0].url_hf = "https://huggingface.co/r/x/resolve/main/w.onnx".to_string();
        assert!(
            validate(&manifest(vec![floating])).is_err(),
            "не прибит к ревизии"
        );
    }

    #[test]
    fn validator_rejects_path_like_ids() {
        for id in ["../m", "a/b", "a\\b", "/m", "CON", "m.new", "x.old-1", ""] {
            let mut broken = entry();
            broken.id = id.to_string();
            assert!(
                validate(&manifest(vec![broken])).is_err(),
                "id {id:?} должен быть отклонён"
            );
        }
    }

    #[test]
    fn legacy_manifest_without_order_and_upstream_loads() {
        let text = r#"{"models":[{"id":"m","title":"t","version":"1","files":[]}]}"#;
        let parsed: ModelsManifest = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.models[0].order, 1, "старый манифест = первая версия");
        assert_eq!(parsed.models[0].kind, "asr");
        assert!(parsed.models[0].upstream.revision.is_empty());
    }
}
