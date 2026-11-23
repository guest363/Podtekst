use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

mod download;
mod install;
mod manifest;
mod state;
mod status;

pub use download::{download_asset, ProgressEvent};
pub use install::{check_space, download_or_update_model, recover_interrupted, required_space};
pub use manifest::{
    bundled, load_bundled_manifest, manifest_updated_at, validate, ModelEntry, ModelFile,
    ModelsManifest, Upstream,
};
pub use state::{load_state, save_state, InstalledFile, InstalledInfo, ModelsState};
pub use status::{get_models_status, ModelStatus};

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
    #[error("нужно {need_bytes} байт, свободно {free_bytes}")]
    NoSpace { need_bytes: u64, free_bytes: u64 },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ModelsError>;

pub fn models_dir(app_data: &Path) -> PathBuf {
    let d = app_data.join("models");
    let _ = fs::create_dir_all(&d);
    d
}

pub fn sha256_of_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    // 64 КиБ вместо 1 МиБ на стеке: главный поток на Windows ограничен 1 МиБ.
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Контрольная сумма ещё не подставлена (манифест в разработке).
pub(crate) fn is_placeholder_sha(sha: &str) -> bool {
    sha.is_empty() || sha.starts_with("REPLACE_")
}

// Минимальный hex-encode без внешних зависимостей (для sha2::Digest output)
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
pub(crate) mod testdir {
    use std::path::PathBuf;

    /// Уникальный каталог на процесс и тест: тесты идут параллельно.
    pub fn new(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("mr-models-{tag}-{}-{nanos}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupted_rejected_by_sha() {
        let tmp = testdir::new("sha");
        let f = tmp.join("x.bin");
        fs::write(&f, b"corrupted").unwrap();
        let h = sha256_of_file(&f).unwrap();
        assert_ne!(h, "00".repeat(32));
        assert!(!is_placeholder_sha(&h));
        assert!(is_placeholder_sha("REPLACE_WITH_REAL"));
    }

    #[test]
    fn manifest_date_is_exposed_for_api() {
        assert_eq!(manifest_updated_at(), bundled().updated_at);
        assert!(!manifest_updated_at().is_empty());
    }

    #[test]
    fn bundled_and_cloned_manifest_agree() {
        assert_eq!(
            load_bundled_manifest().models.len(),
            bundled().models.len(),
            "копия манифеста не должна терять модели"
        );
    }
}
