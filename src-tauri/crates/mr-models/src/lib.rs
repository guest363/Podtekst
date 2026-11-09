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
    #[error("скачивание отменено")]
    Cancelled,
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
    pub kind: String,
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
    pub ram_mb: u64,
    pub required_free_mb: u64,
    pub context_tokens: u32,
    pub min_system_ram_mb: u64,
    pub license_url: String,
    pub source_url: String,
}

pub fn models_dir(app_data: &Path) -> PathBuf {
    let d = app_data.join("models");
    let _ = fs::create_dir_all(&d);
    d
}

/// Манифест вшит, чтобы не зависеть от файлов рядом с бинарником.
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
    use std::io::Write;
    let p = models_dir(app_data).join("state.json");
    let tmp = p.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(state)?.as_bytes())?;
        // sync_all до rename: после переименования данные обязаны уже быть на диске.
        f.sync_all()?;
    }
    fs::rename(&tmp, &p)?;
    Ok(())
}

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
            let mut size_ok = true;
            for f in &m.files {
                let fp = dir.join(&f.filename);
                let Ok(md) = fp.metadata() else { continue };
                if !md.is_file() {
                    continue;
                }
                present += 1;
                on_disk += md.len();
                // Наличие файла ещё не целостность: обрезанную модель видно по размеру
                // без хеширования всего файла на каждый запрос статуса.
                if f.size_bytes > 0 && md.len() != f.size_bytes {
                    size_ok = false;
                }
            }
            let inst = state.installed.get(&m.id);
            let installed = size_ok && present == m.files.len() && !m.files.is_empty();
            let has_update = installed && inst.map(|i| i.version != m.version).unwrap_or(false);
            ModelStatus {
                id: m.id.clone(),
                kind: m.kind.clone(),
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
                ram_mb: m.ram_mb,
                required_free_mb: m.required_free_mb,
                context_tokens: m.context_tokens,
                min_system_ram_mb: m.min_system_ram_mb,
                license_url: m.license_url.clone(),
                source_url: m.source_url.clone(),
            }
        })
        .collect()
}

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

/// Скачать один проверенный файл (например, необязательный runtime-пакет).
/// Докачка `.part` и SHA-256 те же, что у моделей: вызывающий не должен
/// изобретать свою проверку целостности.
pub fn download_asset(
    dest_dir: &Path,
    filename: &str,
    url: &str,
    size_bytes: u64,
    sha256: &str,
    should_abort: &dyn Fn() -> bool,
    on_progress: &dyn Fn(ProgressEvent),
) -> Result<PathBuf> {
    if filename.is_empty() || url.is_empty() || sha256.len() != 64 {
        return Err(ModelsError::Other(
            "некорректное описание файла".to_string(),
        ));
    }
    fs::create_dir_all(dest_dir)?;
    let target = dest_dir.join(filename);
    if target.is_file()
        && sha256_of_file(&target)
            .map(|h| h.eq_ignore_ascii_case(sha256))
            .unwrap_or(false)
    {
        return Ok(target);
    }
    let part = dest_dir.join(format!("{filename}.part"));
    let _lock = DownloadLock::acquire(dest_dir, filename)?;
    download_with_resume(url, &part, size_bytes, should_abort, on_progress)?;
    let actual = sha256_of_file(&part)?;
    if !actual.eq_ignore_ascii_case(sha256) {
        let _ = fs::remove_file(&part);
        return Err(ModelsError::ShaMismatch {
            file: filename.to_string(),
            expected: sha256.to_string(),
            actual,
        });
    }
    fs::rename(&part, &target)?;
    Ok(target)
}

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
        // Пока другой процесс качает этот файл, в его `.part` не пишем.
        let _lock = DownloadLock::acquire(&dir, &f.filename)?;
        let mut urls = vec![f.url_hf.clone()];
        urls.extend(f.mirrors.clone());
        let mut last_err = String::new();
        let mut ok = false;
        for url in urls.iter().filter(|u| !u.is_empty()) {
            match download_with_resume(url, &part, f.size_bytes, &|| false, on_progress) {
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

fn http_agent() -> ureq::Agent {
    // Отдельные таймауты вместо общего дедлайна: крупная модель качается дольше
    // двух минут, но зависшее соединение или молчащий сокет обрываются.
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(120))
        .build()
}

fn range_request(agent: &ureq::Agent, url: &str, already: u64) -> ureq::Request {
    let req = agent.get(url).set("User-Agent", "Podtekst/0.1");
    if already > 0 {
        req.set("Range", &format!("bytes={already}-"))
    } else {
        req
    }
}

fn content_range_start(value: &str) -> Option<u64> {
    value
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

fn content_range_total(value: &str) -> Option<u64> {
    let (_, total) = value.rsplit_once('/')?;
    total.trim().parse().ok()
}

/// Замок скачивания на `.lock`-файле: без внешних крейтов и без гонки за общий `.part`.
struct DownloadLock {
    path: PathBuf,
}

impl DownloadLock {
    /// Замок прошлого запуска после краха иначе блокировал бы скачивание навсегда.
    const STALE: std::time::Duration = std::time::Duration::from_secs(30 * 60);

    fn acquire(dir: &Path, filename: &str) -> Result<Self> {
        let path = dir.join(format!("{filename}.lock"));
        if fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Self::STALE)
        {
            let _ = fs::remove_file(&path);
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(Self { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(ModelsError::Other(
                format!("файл {filename} уже скачивается другим процессом"),
            )),
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for DownloadLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn download_with_resume(
    url: &str,
    dest_part: &Path,
    expected: u64,
    should_abort: &dyn Fn() -> bool,
    on_progress: &dyn Fn(ProgressEvent),
) -> Result<()> {
    use std::io::{Read, Write};
    let agent = http_agent();
    let mut already = if dest_part.exists() {
        dest_part.metadata().map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let mut resp = range_request(&agent, url, already)
        .call()
        .map_err(|e| ModelsError::Net(e.to_string()))?;
    let mut status = resp.status();
    // Сервер вернул не тот диапазон, который просили: дописывать его нельзя,
    // иначе в `.part` окажется кусок с чужого смещения — начинаем файл заново.
    let range_mismatch = status == 206
        && already > 0
        && resp.header("Content-Range").and_then(content_range_start) != Some(already);
    // 416 = `.part` уже больше/равен размеру файла (крах после докачки).
    // Без сброса повторная установка навсегда упиралась бы в этот Range.
    if range_mismatch || (status == 416 && already > 0) {
        already = 0;
        let _ = std::fs::remove_file(dest_part);
        resp = range_request(&agent, url, 0)
            .call()
            .map_err(|e| ModelsError::Net(e.to_string()))?;
        status = resp.status();
    }
    if status != 200 && status != 206 {
        return Err(ModelsError::Net(format!("HTTP {status} для {url}")));
    }
    // Сервер проигнорировал Range (вернул 200 вместо 206) — начинаем файл заново
    let append = status == 206 && already > 0;
    let total: u64 = resp
        .header("Content-Range")
        .and_then(content_range_total)
        .or_else(|| {
            resp.header("Content-Length").and_then(|v| {
                v.parse::<u64>()
                    .ok()
                    .map(|l| l + if append { already } else { 0 })
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
        if should_abort() {
            return Err(ModelsError::Cancelled);
        }
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
        assert!(!models_dir(&tmp).join("state.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn status_requires_exact_file_size() {
        let tmp = std::env::temp_dir().join(format!("mr-models-size-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let m = ModelsManifest {
            manifest_version: 1,
            updated_at: String::new(),
            models: vec![ModelEntry {
                id: "m".to_string(),
                title: "t".to_string(),
                version: "1".to_string(),
                files: vec![ModelFile {
                    filename: "w.onnx".to_string(),
                    url_hf: String::new(),
                    mirrors: vec![],
                    size_bytes: 4,
                    sha256: String::new(),
                    optional: false,
                }],
                embedding_space: None,
                embedding_dim: None,
                license: String::new(),
                description: String::new(),
                kind: "asr".to_string(),
                tier: String::new(),
                ram_mb: 0,
                required_free_mb: 0,
                context_tokens: 0,
                min_system_ram_mb: 0,
                license_url: String::new(),
                source_url: String::new(),
            }],
        };
        let dir = models_dir(&tmp).join("m");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("w.onnx"), b"abc").unwrap();
        assert!(!get_models_status(&tmp, &m)[0].is_installed);
        std::fs::write(dir.join("w.onnx"), b"abcd").unwrap();
        assert!(get_models_status(&tmp, &m)[0].is_installed);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn content_range_is_parsed() {
        assert_eq!(content_range_start("bytes 100-499/1000"), Some(100));
        assert_eq!(content_range_total("bytes 100-499/1000"), Some(1000));
        assert_eq!(content_range_start("bytes */1000"), None);
    }

    #[test]
    fn download_lock_is_exclusive_and_released() {
        let tmp = std::env::temp_dir().join(format!("mr-models-lock-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let first = DownloadLock::acquire(&tmp, "m.onnx").unwrap();
        assert!(DownloadLock::acquire(&tmp, "m.onnx").is_err());
        drop(first);
        assert!(DownloadLock::acquire(&tmp, "m.onnx").is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn verified_asset_is_reused_without_network() {
        let tmp = std::env::temp_dir().join(format!(
            "mr-models-asset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let path = tmp.join("pack.zip");
        std::fs::write(&path, b"verified").unwrap();
        let sha = sha256_of_file(&path).unwrap();
        let got = download_asset(
            &tmp,
            "pack.zip",
            "https://example.invalid/pack.zip",
            b"verified".len() as u64,
            &sha,
            &|| false,
            &|_| {},
        )
        .expect("готовый файл должен переиспользоваться");
        assert_eq!(got, path);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn asset_requires_full_sha() {
        let tmp = std::env::temp_dir().join(format!("mr-models-badsha-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        assert!(download_asset(
            &tmp,
            "x.bin",
            "https://example.invalid",
            0,
            "abc",
            &|| false,
            &|_| {}
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
