pub mod manifest;
pub mod people;
pub mod transfer;

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

pub use manifest::{
    create_pack, create_pack_with_space, delete_pack, ensure_default_pack, get_active_packs,
    get_pack_manifest, list_packs, pack_info_of, set_pack_enabled,
};
pub use people::{add_person_to_pack, move_person_pack, remove_person_from_pack};
pub use transfer::{export_pack, import_pack, import_pack_with_space};

#[derive(Debug, thiserror::Error)]
pub enum PacksError {
    #[error("пакет '{0}' не найден")]
    NotFound(String),
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),
    #[error("ошибка JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ошибка архива: {0}")]
    Zip(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PacksError>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackManifest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created_at: String,
}

fn default_version() -> u32 {
    2
}
fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackInfo {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub enabled: bool,
    pub description: String,
    pub created_at: String,
    pub path: PathBuf,
    #[serde(default)]
    pub people_count: usize,
    #[serde(default)]
    pub voiceprints_count: usize,
    #[serde(default)]
    pub snippets_count: usize,
}

fn now_epoch_secs() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

pub(crate) fn slugify(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_dash = false;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        format!("pack-{}", now_epoch_secs())
    } else {
        s
    }
}

pub fn packs_root_dir(app_data: &Path) -> PathBuf {
    let root = app_data.join("packs");
    let _ = fs::create_dir_all(&root);
    root
}

pub fn default_app_data_dir() -> PathBuf {
    if let Ok(v) =
        std::env::var("PODTEKST_DATA_DIR").or_else(|_| std::env::var("MEETING_RECORDER_DATA_DIR"))
    {
        let p = PathBuf::from(v);
        let _ = fs::create_dir_all(&p);
        return p;
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let base = PathBuf::from(home)
                .join("Library")
                .join("Application Support");
            let p = base.join("Podtekst");
            let old = base.join("MeetingRecorder");
            if !p.exists() && old.exists() {
                let _ = fs::rename(&old, &p);
            }
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let base = PathBuf::from(appdata);
            let p = base.join("Podtekst");
            let old = base.join("MeetingRecorder");
            if !p.exists() && old.exists() {
                let _ = fs::rename(&old, &p);
            }
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local").join("share")
        });
    let p = base.join("podtekst");
    let old = base.join("meeting-recorder");
    if !p.exists() && old.exists() {
        let _ = fs::rename(&old, &p);
    }
    let _ = fs::create_dir_all(&p);
    p
}

/// Идентификатор пакета валиден, если это одиночный компонент пути без `..`.
/// Единая защита от path traversal для всех операций с пакетами.
pub(crate) fn validate_pack_id(pack_id: &str) -> Result<&str> {
    if pack_id.is_empty()
        || pack_id.contains('/')
        || pack_id.contains('\\')
        || pack_id.contains("..")
        || pack_id == "."
    {
        return Err(PacksError::Other(format!(
            "недопустимый идентификатор пакета: {pack_id:?}"
        )));
    }
    Ok(pack_id)
}

/// Путь сниппета из `voiceprints.json` (в том числе импортированного пакета):
/// только относительный путь из обычных компонентов.
pub(crate) fn safe_snippet_rel(rel: &str) -> Option<&Path> {
    let path = Path::new(rel);
    if rel.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(path)
}

/// Сниппет существует и после канонизации лежит внутри каталога пакета.
pub fn safe_snippet_path(pack_dir: &Path, rel: &str) -> Option<PathBuf> {
    let rel = safe_snippet_rel(rel)?;
    let root = pack_dir.canonicalize().ok()?;
    let candidate = pack_dir.join(rel).canonicalize().ok()?;
    (candidate.starts_with(&root) && candidate.is_file()).then_some(candidate)
}

/// Атомарная запись JSON (tmp + rename): крах процесса не оставляет битый файл.
pub(crate) fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    // Уникальное имя: общий `.tmp` перемешал бы байты двух записей и оставил половину JSON.
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(value)?;
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        // sync_all до rename: после переименования данные обязаны уже быть на диске.
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Отпечатки сразу в каноническом формате v3. Пространство пишем только когда
/// оно известно: пустая строка означала бы «чужое» и отбросила бы все профили.
pub(crate) fn voiceprints_v3(
    profiles: Vec<serde_json::Value>,
    embedding_space: &str,
) -> serde_json::Value {
    let mut value = serde_json::json!({"version": 3, "profiles": profiles});
    if !embedding_space.is_empty() {
        value["embedding_space"] = serde_json::json!(embedding_space);
    }
    value
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    /// Временный каталог, удаляемый при падении Guard (общий для тестов крейта).
    pub struct Guard(pub PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn tmp_app(tag: &str) -> (Guard, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mr-packs-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let guard = Guard(dir.clone());
        (guard, dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tmp_app;

    #[test]
    fn concurrent_json_writes_leave_valid_file() {
        let (_g, app) = tmp_app("atomic-json");
        let path = app.join("people.json");
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..20 {
                        let value = vec![serde_json::json!({"name": format!("w{worker}-{i}")})];
                        write_json_atomic(&path, &value).expect("запись");
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("поток записи");
        }
        let text = fs::read_to_string(&path).expect("файл на месте");
        let value: serde_json::Value = serde_json::from_str(&text)
            .expect("после параллельных записей файл должен быть валидным JSON");
        assert!(value.as_array().is_some_and(|items| items.len() == 1));
    }
}
