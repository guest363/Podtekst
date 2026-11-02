use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// fsync до rename: иначе после сбоя питания можно получить пустой или битый файл.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = std::env::temp_dir().join(format!("mr-fsutil-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
