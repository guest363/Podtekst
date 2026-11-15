use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{models_dir, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFile {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstalledInfo {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub order: Option<u32>,
    #[serde(default)]
    pub files: Vec<InstalledFile>,
    #[serde(default)]
    pub source_revision: Option<String>,
    #[serde(default)]
    pub embedding_space: Option<String>,
    #[serde(default)]
    pub installed_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelsState {
    #[serde(default)]
    pub installed: HashMap<String, InstalledInfo>,
}

/// Глобальный лок состояния: чтение-изменение-запись `state.json` из двух
/// процессов (приложение и CLI) иначе теряет записи.
pub(crate) struct StateLock {
    file: fs::File,
}

impl StateLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(root.join(".state.lock"))?;
        FileExt::lock_exclusive(&file)?;
        Ok(Self { file })
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Прочитать `state.json` (отсутствие/битый файл = состояние по умолчанию).
pub fn load_state(app_data: &Path) -> ModelsState {
    load_state_in(&models_dir(app_data))
}

pub(crate) fn load_state_in(root: &Path) -> ModelsState {
    fs::read_to_string(root.join("state.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Сохранить `state.json` атомарно (tmp + rename): крах не оставляет битый файл.
pub fn save_state(app_data: &Path, state: &ModelsState) -> Result<()> {
    save_state_in(&models_dir(app_data), state)
}

pub(crate) fn save_state_in(root: &Path, state: &ModelsState) -> Result<()> {
    let tmp = root.join(format!("state.json.tmp.{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(state)?.as_bytes())?;
        // sync_all до rename: после переименования данные обязаны уже быть на диске.
        f.sync_all()?;
    }
    fs::rename(&tmp, root.join("state.json"))?;
    // Каталог синхронизируем best-effort: на Windows дескриптор каталога не открыть.
    if let Ok(dir) = fs::File::open(root) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Чтение-изменение-запись под глобальным локом.
pub(crate) fn update_state(root: &Path, apply: impl FnOnce(&mut ModelsState)) -> Result<()> {
    let _lock = StateLock::acquire(root)?;
    let mut state = load_state_in(root);
    apply(&mut state);
    save_state_in(root, &state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir;

    #[test]
    fn state_save_is_atomic_roundtrip() {
        let tmp = testdir::new("state");
        let mut st = ModelsState::default();
        st.installed.insert(
            "x".to_string(),
            InstalledInfo {
                version: "1".to_string(),
                installed_at: String::new(),
                ..Default::default()
            },
        );
        save_state(&tmp, &st).unwrap();
        assert_eq!(load_state(&tmp).installed.len(), 1);
        let root = models_dir(&tmp);
        assert!(!root
            .join(format!("state.json.tmp.{}", std::process::id()))
            .exists());
        assert!(
            !root.join("state.json.tmp").exists(),
            "старое имя временного файла"
        );
    }

    #[test]
    fn legacy_record_without_files_and_order_loads() {
        let st: ModelsState = serde_json::from_str(
            r#"{"installed":{"m":{"version":"1","embedding_space":null,"installed_at":""}}}"#,
        )
        .unwrap();
        let info = &st.installed["m"];
        assert!(info.files.is_empty());
        assert!(info.order.is_none());
        assert!(info.source_revision.is_none());
    }

    #[test]
    fn concurrent_updates_keep_both_records() {
        let tmp = testdir::new("state-race");
        let root = models_dir(&tmp);
        let threads: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|id| {
                let root = root.clone();
                std::thread::spawn(move || {
                    update_state(&root, |state| {
                        state
                            .installed
                            .insert(id.to_string(), InstalledInfo::default());
                    })
                    .unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let state = load_state(&tmp);
        assert_eq!(state.installed.len(), 2, "один из потоков потерял запись");
    }
}
