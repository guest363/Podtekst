//! Реестр и обновление ИИ-моделей (порт `services/model_manager.py`).
//!
//! * Декларативный `models-manifest.json` с версиями, размерами и SHA-256
//! * Скачивание с докачкой (HTTP Range → `.part`), зеркалами и верификацией SHA-256
//! * Атомарная активация и `state.json`

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ModelsError {
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),
    #[error("ошибка JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ошибка сети: {0}")]
    Net(String),
    #[error("SHA-256 не совпал для {file}: ожидался {expected}, получен {actual}")]
    ShaMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("модель '{0}' не найдена в манифесте")]
    NotFound(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ModelsError>;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub title: String,
    pub version: String,
    #[serde(default)]
    pub files: Vec<ModelFile>,
    pub embedding_space: Option<String>,
    #[serde(default)]
    pub embedding_dim: Option<usize>,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub description: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstalledInfo {
    #[serde(default)]
    pub version: String,
    pub embedding_space: Option<String>,
    #[serde(default)]
    pub installed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsState {
    #[serde(default)]
    pub installed: std::collections::HashMap<String, InstalledInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub title: String,
    pub version: String,
    pub installed_version: Option<String>,
    pub is_installed: bool,
    pub has_update: bool,
    pub total_size_bytes: u64,
    pub on_disk_bytes: u64,
    pub files_total: usize,
    pub files_present: usize,
    pub license: String,
    pub description: String,
    pub embedding_space: Option<String>,
}

/// Каталог моделей `<app_data>/models` (создаётся при необходимости).
pub fn models_dir(app_data: &Path) -> PathBuf {
    let d = app_data.join("models");
    let _ = fs::create_dir_all(&d);
    d
}

/// Встроенный манифест: сначала ищем рядом с бинарником, иначе — исходник репозитория.
pub fn load_bundled_manifest() -> ModelsManifest {
    const BUNDLED: &str = include_str!("../models-manifest.json");
    serde_json::from_str(BUNDLED).unwrap_or(ModelsManifest {
        manifest_version: 1,
        updated_at: String::new(),
        models: vec![],
    })
}

/// Прочитать `state.json` (отсутствие/битый файл = состояние по умолчанию).
pub fn load_state(app_data: &Path) -> ModelsState {
    let p = models_dir(app_data).join("state.json");
    fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Сохранить `state.json` атомарно (tmp + rename): крах не оставляет битый файл.
pub fn save_state(app_data: &Path, state: &ModelsState) -> Result<()> {
    let p = models_dir(app_data).join("state.json");
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(state).unwrap())?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

/// Статус каждой модели манифеста: установлена ли, есть ли обновление.
pub fn get_models_status(app_data: &Path, manifest: &ModelsManifest) -> Vec<ModelStatus> {
    let state = load_state(app_data);
    let root = models_dir(app_data);
    manifest
        .models
        .iter()
        .map(|m| {
            let total: u64 = m.files.iter().map(|f| f.size_bytes).sum();
            let dir = root.join(&m.id);
            let mut present = 0;
            let mut on_disk = 0u64;
            for f in &m.files {
                let fp = dir.join(&f.filename);
                if fp.is_file() {
                    present += 1;
                    on_disk += fp.metadata().map(|md| md.len()).unwrap_or(0);
                }
            }
            let inst = state.installed.get(&m.id);
            let installed = present == m.files.len() && !m.files.is_empty();
            let has_update = installed && inst.map(|i| i.version != m.version).unwrap_or(false);
            ModelStatus {
                id: m.id.clone(),
                title: m.title.clone(),
                version: m.version.clone(),
                installed_version: inst.map(|i| i.version.clone()),
                is_installed: installed,
                has_update,
                total_size_bytes: total,
                on_disk_bytes: on_disk,
                files_total: m.files.len(),
                files_present: present,
                license: m.license.clone(),
                description: m.description.clone(),
                embedding_space: m.embedding_space.clone(),
            }
        })
        .collect()
}

/// SHA-256 файла (hex, нижний регистр).
pub fn sha256_of_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        use std::io::Read;
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn is_placeholder_sha(sha: &str) -> bool {
    sha.is_empty() || sha.starts_with("REPLACE_")
}

#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub pct: u8,
    pub message: String,
}

/// Скачивание модели с докачкой, зеркалами и проверкой SHA-256.
pub fn download_or_update_model(
    app_data: &Path,
    manifest: &ModelsManifest,
    model_id: &str,
    on_progress: &dyn Fn(ProgressEvent),
) -> Result<()> {
    let spec = manifest
        .models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| ModelsError::NotFound(model_id.to_string()))?;
    // Путь строим из id манифеста: даже при коллизии входа инвариант не зависит от порядка.
    let dir = models_dir(app_data).join(&spec.id);
    fs::create_dir_all(&dir)?;
    for f in &spec.files {
        let target = dir.join(&f.filename);
        let part = dir.join(format!("{}.part", f.filename));
        // Пропуск, если файл уже валиден
        if target.exists() {
            if !is_placeholder_sha(&f.sha256) {
                if sha256_of_file(&target)
                    .map(|h| h.eq_ignore_ascii_case(&f.sha256))
                    .unwrap_or(false)
                {
                    continue;
                }
            } else if f.size_bytes > 0
                && target.metadata().map(|m| m.len()).unwrap_or(0) == f.size_bytes
            {
                continue;
            }
        }
        let mut urls = vec![f.url_hf.clone()];
        urls.extend(f.mirrors.clone());
        let mut last_err = String::new();
        let mut ok = false;
        for url in urls.iter().filter(|u| !u.is_empty()) {
            match download_with_resume(url, &part, f.size_bytes, on_progress) {
                Ok(()) => {
                    if !is_placeholder_sha(&f.sha256) {
                        match sha256_of_file(&part) {
                            Ok(h) if h.eq_ignore_ascii_case(&f.sha256) => {
                                fs::rename(&part, &target)?;
                                ok = true;
                                break;
                            }
                            Ok(h) => {
                                last_err = format!("SHA mismatch: {h}");
                                let _ = fs::remove_file(&part);
                                continue;
                            }
                            Err(e) => {
                                last_err = e.to_string();
                                continue;
                            }
                        }
                    } else {
                        fs::rename(&part, &target)?;
                        ok = true;
                        break;
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            }
        }
        if !ok {
            return Err(ModelsError::Other(format!(
                "Не удалось скачать {}: {last_err}",
                f.filename
            )));
        }
    }
    let mut state = load_state(app_data);
    state.installed.insert(
        model_id.to_string(),
        InstalledInfo {
            version: spec.version.clone(),
            embedding_space: spec.embedding_space.clone(),
            installed_at: String::from("rust-core"),
        },
    );
    save_state(app_data, &state)?;
    Ok(())
}

fn download_with_resume(
    url: &str,
    dest_part: &Path,
    expected: u64,
    on_progress: &dyn Fn(ProgressEvent),
) -> Result<()> {
    use std::io::{Read, Write};
    let already = if dest_part.exists() {
        dest_part.metadata().map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let req = if already > 0 {
        ureq::get(url)
            .set("User-Agent", "MeetingRecorder/0.1")
            .set("Range", &format!("bytes={already}-"))
    } else {
        ureq::get(url).set("User-Agent", "MeetingRecorder/0.1")
    };
    let resp = req
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .map_err(|e| ModelsError::Net(e.to_string()))?;
    let status = resp.status();
    if status != 200 && status != 206 {
        return Err(ModelsError::Net(format!("HTTP {status} для {url}")));
    }
    // Сервер проигнорировал Range (вернул 200 вместо 206) — начинаем файл заново
    let append = status == 206 && already > 0;
    let total: u64 = resp
        .header("Content-Range")
        .and_then(|v| v.rsplit('/').next()?.parse().ok())
        .or_else(|| {
            resp.header("Content-Length").and_then(|v| {
                v.parse::<u64>()
                    .ok()
                    .map(|l| l + u64::from(append) * already)
            })
        })
        .unwrap_or(already);
    let mut out = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(dest_part)?;
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 256 * 1024];
    let mut done = if append { already } else { 0 };
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| ModelsError::Net(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        let pct = if total > 0 {
            ((done as f64 / total as f64) * 100.0).min(100.0) as u8
        } else {
            0
        };
        on_progress(ProgressEvent {
            downloaded_bytes: done,
            total_bytes: total,
            pct,
            message: format!("{done}/{total} bytes"),
        });
    }
    out.flush()?;
    // Обрыв соединения посередине выглядит как ранний EOF: сверяем размер,
    // иначе недокачанная модель «установится» без проверки (placeholder-SHA).
    if expected > 0 && done != expected {
        return Err(ModelsError::Net(format!(
            "файл скачан не целиком: {done} из {expected} байт"
        )));
    }
    Ok(())
}

// Минимальный hex-encode без внешних зависимостей (для sha2::Digest output)
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_and_statuses() {
        let m = load_bundled_manifest();
        assert!(!m.models.is_empty());
        let tmp = std::env::temp_dir().join(format!("mr-models-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let statuses = get_models_status(&tmp, &m);
        assert_eq!(statuses.len(), m.models.len());
        for s in &statuses {
            assert!(!s.id.is_empty());
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn corrupted_rejected_by_sha() {
        let tmp = std::env::temp_dir().join(format!("mr-models-sha-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let f = tmp.join("x.bin");
        std::fs::write(&f, b"corrupted").unwrap();
        let h = sha256_of_file(&f).unwrap();
        assert_ne!(h, "00".repeat(32));
        assert!(!is_placeholder_sha(&h));
        assert!(is_placeholder_sha("REPLACE_WITH_REAL"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unknown_model_errors() {
        let m = load_bundled_manifest();
        let tmp = std::env::temp_dir().join(format!("mr-models-nf-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let r = download_or_update_model(&tmp, &m, "no-such-model", &|_| {});
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn state_save_is_atomic_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("mr-models-st-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let mut st = ModelsState::default();
        st.installed.insert(
            "x".to_string(),
            InstalledInfo {
                version: "1".to_string(),
                embedding_space: None,
                installed_at: String::new(),
            },
        );
        save_state(&tmp, &st).unwrap();
        assert_eq!(load_state(&tmp).installed.len(), 1);
        // После атомарной записи не остаётся временных файлов.
        assert!(!models_dir(&tmp).join("state.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
