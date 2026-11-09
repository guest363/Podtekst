use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Достаёт ровно один файл из архива: второй копии llama-server не нужно,
/// пакет — только backend-библиотека.
pub(super) fn extract_library(
    archive: &Path,
    format: &str,
    library: &str,
    dest: &Path,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("папка пакета: {e}"))?;
    let target = dest.join(library);
    let part = dest.join(format!("{library}.part"));
    let _ = std::fs::remove_file(&part);
    match format {
        "tar.gz" => extract_tar(archive, library, &part)?,
        "zip" => extract_zip(archive, library, &part)?,
        other => return Err(format!("неизвестный формат пакета: {other}")),
    }
    std::fs::rename(&part, &target).map_err(|e| format!("фиксация пакета: {e}"))?;
    Ok(target)
}

pub(super) fn safe_entry(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\0')
        && !name.starts_with('/')
        && !name.starts_with('-')
        && !Path::new(name).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn extract_tar(archive: &Path, library: &str, target: &Path) -> Result<(), String> {
    let list = Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .map_err(|e| format!("tar недоступен: {e}"))?;
    if !list.status.success() {
        return Err("архив не читается tar".to_string());
    }
    let entry = String::from_utf8_lossy(&list.stdout)
        .lines()
        .find(|line| {
            safe_entry(line)
                && Path::new(line).file_name().and_then(|n| n.to_str()) == Some(library)
        })
        .map(str::to_string)
        .ok_or_else(|| format!("в архиве нет {library}"))?;
    let tmp = target.with_extension("extract");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("временная папка: {e}"))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&tmp)
        .arg(&entry)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("распаковка tar не удалась".to_string());
    }
    let src = tmp.join(&entry);
    std::fs::copy(&src, target).map_err(|e| format!("копия {library}: {e}"))?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

fn extract_zip(archive: &Path, library: &str, target: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("архив: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("zip: {e}"))?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| format!("zip entry: {e}"))?;
        let raw = entry.name();
        if raw.contains('\\') || raw.contains(':') || raw.contains('\0') {
            continue;
        }
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        if path.file_name().and_then(|n| n.to_str()) != Some(library) {
            continue;
        }
        // Симлинк в zip на Windows не нужен и опасен: файл пакета — обычный файл.
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(format!("{library} — симлинк, пакет отклонён"));
        }
        let mut out = std::fs::File::create(target).map_err(|e| format!("файл пакета: {e}"))?;
        let mut limited = entry.by_ref().take(128 * 1024 * 1024);
        std::io::copy(&mut limited, &mut out).map_err(|e| format!("распаковка zip: {e}"))?;
        out.flush().map_err(|e| format!("запись пакета: {e}"))?;
        return Ok(());
    }
    Err(format!("в архиве нет {library}"))
}
