use std::fs;
use std::path::{Path, PathBuf};

use crate::{sha256_of_file, ModelsError, Result};

#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub pct: u8,
    pub message: String,
}

/// Каталог недокачанных файлов: `.part` и `.lock` не должны попадать в каталог
/// модели, иначе загрузчик примет их за готовые веса.
pub(crate) fn tmp_dir(dir: &Path) -> PathBuf {
    dir.join(".tmp")
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
    if target.is_file() && file_matches(&target, size_bytes, sha256) {
        return Ok(target);
    }
    let tmp = tmp_dir(dest_dir);
    fs::create_dir_all(&tmp)?;
    let part = tmp.join(format!("{filename}.part"));
    let _lock = DownloadLock::acquire(&tmp, filename)?;
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
    // Windows не переименовывает поверх существующего файла.
    let _ = fs::remove_file(&target);
    fs::rename(&part, &target)?;
    Ok(target)
}

pub(crate) fn file_matches(path: &Path, size_bytes: u64, sha256: &str) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if size_bytes > 0 && meta.len() != size_bytes {
        return false;
    }
    if crate::is_placeholder_sha(sha256) {
        return true;
    }
    sha256_of_file(path)
        .map(|h| h.eq_ignore_ascii_case(sha256))
        .unwrap_or(false)
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

/// Замок скачивания на `.lock`-файле: ОС снимает блокировку сама при падении
/// процесса, поэтому «протухших» замков не бывает и второй писатель в `.part`
/// не попадает даже на многогигабайтном файле с медленного канала.
pub(crate) struct DownloadLock {
    _file: fs::File,
}

impl DownloadLock {
    pub(crate) fn acquire(dir: &Path, filename: &str) -> Result<Self> {
        let path = dir.join(format!("{filename}.lock"));
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(ModelsError::Other(format!(
                "файл {filename} уже скачивается другим процессом"
            ))),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

pub(crate) fn download_with_resume(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir;

    #[test]
    fn content_range_is_parsed() {
        assert_eq!(content_range_start("bytes 100-499/1000"), Some(100));
        assert_eq!(content_range_total("bytes 100-499/1000"), Some(1000));
        assert_eq!(content_range_start("bytes */1000"), None);
    }

    #[test]
    fn download_lock_is_exclusive_and_released() {
        let tmp = testdir::new("lock");
        let first = DownloadLock::acquire(&tmp, "m.onnx").unwrap();
        assert!(DownloadLock::acquire(&tmp, "m.onnx").is_err());
        drop(first);
        assert!(DownloadLock::acquire(&tmp, "m.onnx").is_ok());
        assert!(tmp.join("m.onnx.lock").is_file(), "лок лежит рядом с .part");
    }

    #[test]
    fn verified_asset_is_reused_without_network() {
        let tmp = testdir::new("asset");
        let path = tmp.join("pack.zip");
        fs::write(&path, b"verified").unwrap();
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
        assert_eq!(fs::read(&path).unwrap(), b"verified".to_vec());
    }

    #[test]
    fn asset_requires_full_sha() {
        let tmp = testdir::new("badsha");
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
    }
}
