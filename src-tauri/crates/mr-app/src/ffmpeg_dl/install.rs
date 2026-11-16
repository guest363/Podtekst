//! Скачивание, проверка и распаковка архива — всё до публикации в `<bin>`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::pins::Package;

const CHUNK: usize = 256 * 1024;
/// Архив ffmpeg — 40–120 МиБ; 300 МиБ с запасом ловит подменённый CDN.
const MAX_ARCHIVE_BYTES: u64 = 300 * 1024 * 1024;
/// Общий дедлайн: `timeout_read` ограничивает паузу сокета, а не всю загрузку.
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(15 * 60);

pub(super) fn download(
    package: &Package,
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("временная папка: {e}"))?;
    let path = dir.join(format!(
        "{}.{}",
        uuid::Uuid::new_v4(),
        extension(&package.format)
    ));
    if !package.url.starts_with("https://") {
        return Err(format!("пин ссылается не на https: {}", package.url));
    }
    // Медленный канал не должен ронять установку, пока данные идут, поэтому
    // предел ставится на простой сокета, а не на весь архив.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        // Редирект только на https и не больше двух хопов: http-пин не качаем.
        .https_only(true)
        .redirects(2)
        .build();
    let resp = agent
        .get(&package.url)
        .set("User-Agent", "MeetingRecorder/0.1")
        .call()
        .map_err(|e| format!("сеть: {e}"))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if total > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "архив больше {} МиБ: проверьте пин",
            MAX_ARCHIVE_BYTES / 1024 / 1024
        ));
    }
    progress(0, total);

    let mut file = std::fs::File::create_new(&path).map_err(|e| format!("файл: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    let deadline = std::time::Instant::now() + DOWNLOAD_DEADLINE;
    loop {
        if std::time::Instant::now() > deadline {
            let _ = std::fs::remove_file(&path);
            return Err("скачивание не уложилось в 15 минут".to_string());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("чтение: {e}"))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > MAX_ARCHIVE_BYTES {
            let _ = std::fs::remove_file(&path);
            return Err("архив больше допустимого размера".to_string());
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("запись: {e}"))?;
        progress(done, total);
    }
    file.flush().map_err(|e| format!("запись: {e}"))?;
    Ok(path)
}

fn extension(format: &str) -> &'static str {
    if format == "zip" {
        "zip"
    } else {
        "tar.xz"
    }
}

pub(super) fn verify_archive(package: &Package, archive: &Path) -> Result<(), String> {
    let actual = sha256(archive)?;
    if !actual.eq_ignore_ascii_case(&package.sha256) {
        return Err(format!(
            "контрольная сумма архива не совпала: ожидалась {}, получена {actual}",
            package.sha256
        ));
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String, String> {
    mr_models::sha256_of_file(path).map_err(|e| format!("sha256: {e}"))
}

pub(super) fn extract(package: &Package, archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("распаковка: {e}"))?;
    match package.format.as_str() {
        "zip" => unzip(archive, dest),
        "tar.xz" => untar(archive, dest),
        other => Err(format!("неизвестный формат архива: {other}")),
    }
}

fn unzip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("архив: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("zip: {e}"))?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| format!("zip: {e}"))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(format!(
                "недопустимый путь в архиве: {}",
                String::from_utf8_lossy(entry.name_raw())
            ));
        };
        if entry.is_symlink() {
            return Err(format!(
                "ссылка в архиве запрещена: {}",
                String::from_utf8_lossy(entry.name_raw())
            ));
        }
        let out = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("распаковка: {e}"))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("распаковка: {e}"))?;
        }
        let mut sink = std::fs::File::create(&out).map_err(|e| format!("распаковка: {e}"))?;
        std::io::copy(&mut entry, &mut sink).map_err(|e| format!("распаковка: {e}"))?;
    }
    Ok(())
}

fn untar(archive: &Path, dest: &Path) -> Result<(), String> {
    let listing = Command::new("tar")
        .arg("-tvf")
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("tar недоступен: {e}"))?;
    if !listing.status.success() {
        return Err(format!(
            "tar не прочитал архив: {}",
            String::from_utf8_lossy(&listing.stderr).trim()
        ));
    }
    validate_tar_listing(&String::from_utf8_lossy(&listing.stdout))?;
    let extracted = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("tar недоступен: {e}"))?;
    if !extracted.success() {
        return Err("tar не распаковал архив".to_string());
    }
    Ok(())
}

/// Список `tar -tvf`: отказ на абсолютные пути, `..` и ссылки до распаковки.
pub(super) fn validate_tar_listing(listing: &str) -> Result<(), String> {
    for line in listing.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        let kind = line.chars().next().unwrap_or(' ');
        if kind == 'l' || kind == 'h' {
            return Err(format!("ссылка в архиве запрещена: {line}"));
        }
        if line.contains("..") {
            return Err(format!("недопустимый путь в архиве: {line}"));
        }
        if line
            .split_whitespace()
            .any(|token| token.starts_with('/') || token.contains(":\\"))
        {
            return Err(format!("абсолютный путь в архиве: {line}"));
        }
    }
    Ok(())
}

pub(super) fn find_binary(root: &Path, tool: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            // Только точное имя: `ffmpeg-readme.txt` из архива не должен
            // подменять бинарник в зависимости от порядка обхода каталога.
            if name == tool || name == format!("{tool}.exe") {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(format: &str) -> Package {
        Package {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            version: "1.0".to_string(),
            source: "test".to_string(),
            provides: vec!["ffmpeg".to_string()],
            format: format.to_string(),
            url: "https://example.invalid/a".to_string(),
            sha256: "0".repeat(64),
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-ffmpeg-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_zip(path: &Path, name: &str, body: &[u8], symlink: bool) {
        use zip::write::SimpleFileOptions;
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        if symlink {
            writer
                .add_symlink(name, "ffmpeg", SimpleFileOptions::default())
                .unwrap();
        } else {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn tar_listing_rejects_escapes_and_links() {
        assert!(validate_tar_listing(
            "-rwxr-xr-x user/group 12 2025-01-01 00:00 ffmpeg-7.0.2-amd64-static/ffmpeg\n"
        )
        .is_ok());
        assert!(validate_tar_listing("-rw-r--r-- u/g 1 2025-01-01 00:00 ../evil\n").is_err());
        assert!(validate_tar_listing("-rw-r--r-- u/g 1 2025-01-01 00:00 /etc/passwd\n").is_err());
        assert!(
            validate_tar_listing("lrwxrwxrwx u/g 0 2025-01-01 00:00 link -> /etc/passwd\n")
                .is_err()
        );
        assert!(
            validate_tar_listing("hrw-r--r-- u/g 0 2025-01-01 00:00 hard -> ffmpeg\n").is_err()
        );
    }

    #[test]
    fn zip_slip_is_rejected() {
        let dir = temp_dir("slip");
        let archive = dir.join("a.zip");
        write_zip(&archive, "../evil", b"x", false);
        let out = dir.join("out");
        let error = extract(&package("zip"), &archive, &out).expect_err("zip-slip запрещён");
        assert!(error.contains("недопустимый путь"), "{error}");
        assert!(!dir.join("evil").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_symlink_is_rejected() {
        let dir = temp_dir("link");
        let archive = dir.join("a.zip");
        write_zip(&archive, "link", b"", true);
        let error = extract(&package("zip"), &archive, &dir.join("out"))
            .expect_err("ссылка в zip запрещена");
        assert!(error.contains("ссылка"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
