use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// fsync до и после rename: иначе после сбоя питания можно получить пустой
/// файл или потерять саму запись каталога.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, bytes, false)
}

/// Итоги содержат личный текст встречи: файл создаётся 0600 (Unix) и с
/// DACL только для текущего пользователя (Windows), в отличие от записей.
pub(crate) fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, bytes, true)
}

fn write_atomic_with(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    let written = write_tmp(&tmp, bytes, private);
    if let Err(error) = written {
        // Ошибка записи не должна оставлять в папке записей копию текста встречи.
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    if private && !apply_private_acl(path) {
        crate::note!(
            "acl_fallback file={} reason=icacls",
            crate::logging::file_token(path)
        );
    }
    Ok(())
}

fn write_tmp(tmp: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    let mut file = if private {
        let file = private_options().create_new(true).open(tmp)?;
        if !apply_private_acl(tmp) {
            crate::note!(
                "acl_fallback file={} reason=icacls",
                crate::logging::file_token(tmp)
            );
        }
        file
    } else {
        std::fs::File::create(tmp)?
    };
    file.write_all(bytes)?;
    file.sync_all()
}

/// Права «только владелец» для уже созданных файлов (app.log, журнал, реестр).
pub(crate) fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(unix)]
fn private_options() -> std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    // Права задаются до появления файла: иначе на общей машине он успевает
    // побыть доступным для чтения.
    o.mode(0o600);
    o
}

#[cfg(not(unix))]
fn private_options() -> std::fs::OpenOptions {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    o
}

#[cfg(windows)]
fn apply_private_acl(path: &Path) -> bool {
    // icacls дешевле полного SetNamedSecurityInfoW ради двух файлов; DACL
    // ставится до записи, чтобы файл не открылся другим пользователям даже на миг.
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.is_empty() {
        return false;
    }
    let principal = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    };
    let applied = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{principal}:F"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !applied {
        return false;
    }
    std::process::Command::new("icacls")
        .arg(path)
        .output()
        .ok()
        .map(|output| {
            let text = String::from_utf8_lossy(&output.stdout);
            text.contains(&principal)
                && !text.contains("BUILTIN\\Users")
                && !text.contains("Everyone")
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn apply_private_acl(_path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-fsutil-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = temp_dir("atomic");
        let p = dir.join("a.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_write_leaves_no_tmp_file() {
        let dir = temp_dir("no-tmp");
        let p = dir.join("a.json");
        std::fs::create_dir_all(&p).unwrap();
        assert!(write_atomic(&p, b"body").is_err());
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "остались временные файлы");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn private_write_has_user_only_acl() {
        let dir = temp_dir("acl");
        let p = dir.join("s.summary.json");
        write_atomic_private(&p, b"{}").unwrap();
        let user = std::env::var("USERNAME").unwrap_or_default();
        let output = std::process::Command::new("icacls")
            .arg(&p)
            .output()
            .expect("icacls");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains(&user), "DACL без пользователя: {text}");
        assert!(
            !text.contains("BUILTIN\\Users") && !text.contains("Everyone"),
            "DACL остался наследуемым: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn private_write_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("priv");
        let p = dir.join("s.summary.json");
        write_atomic_private(&p, b"{}").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "summary-артефакт должен быть только для владельца"
        );
        write_atomic_private(&p, b"[]").unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
