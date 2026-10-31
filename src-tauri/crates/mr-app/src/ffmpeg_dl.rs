//! Скачивание ffmpeg по кнопке из интерфейса (Настройки → Модели ИИ).
//!
//! Источники — публичные сборки с GitHub: essentials от gyan (меньше размер),
//! затем latest от BtbN как зеркало. Архив распаковывается встроенным `tar`
//! (bsdtar, Windows 10 1803+; запасной путь — PowerShell), бинарники кладутся
//! в `<данные>/bin` и проверяются запуском `-version`. Ядро подхватывает их
//! через `mr_audio::tools::ffmpeg_path()` без пересборки.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const URLS: &[&str] = &[
    "https://github.com/GyanD/codexffmpeg/releases/download/7.1.1/ffmpeg-7.1.1-essentials_build.zip",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
];

/// Страница ручной установки — подсказка в интерфейсе при неудаче автоскачивания.
pub(crate) const MANUAL_URL: &str = "https://www.gyan.dev/ffmpeg/builds/";

#[derive(Default)]
struct Dl {
    done: u64,
    total: u64,
    error: Option<String>,
}

static DL: Mutex<Option<Dl>> = Mutex::new(None);

/// Состояние для `/api/status`: установлен ли ffmpeg, идёт ли скачивание.
pub(crate) fn status_json() -> serde_json::Value {
    let available = mr_audio::tools::ffmpeg_available();
    let guard = DL.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(d) if d.error.is_none() => serde_json::json!({
            "available": available,
            "downloading": true,
            "pct": (d.done * 95).checked_div(d.total).map(|v| (v as u8).min(95)).unwrap_or(0),
            "error": serde_json::Value::Null,
            "manual_url": MANUAL_URL,
        }),
        Some(d) => serde_json::json!({
            "available": available, "downloading": false, "pct": 0, "error": d.error,
            "manual_url": MANUAL_URL,
        }),
        None => serde_json::json!({
            "available": available, "downloading": false, "pct": 0,
            "error": serde_json::Value::Null, "manual_url": MANUAL_URL,
        }),
    }
}

/// Запускает фоновое скачивание и установку ffmpeg.
pub(crate) fn start_download() -> Result<(), String> {
    {
        let mut guard = DL.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|d| d.error.is_none()).unwrap_or(false) {
            return Err("Скачивание уже идёт".to_string());
        }
        *guard = Some(Dl::default());
    }
    std::thread::Builder::new()
        .name("ffmpeg-dl".into())
        .spawn(download_worker)
        .map_err(|e| format!("Не удалось запустить поток скачивания: {e}"))?;
    Ok(())
}

fn set_progress(done: u64, total: u64) {
    if let Ok(mut g) = DL.lock() {
        if let Some(d) = g.as_mut() {
            d.done = done;
            d.total = total;
        }
    }
}

fn download_worker() {
    let err = download_and_install();
    let mut guard = DL.lock().unwrap_or_else(|e| e.into_inner());
    if err.is_none() {
        *guard = None;
    } else if let Some(d) = guard.as_mut() {
        d.error = err;
    }
}

fn download_and_install() -> Option<String> {
    if !cfg!(windows) {
        return Some(
            "Автоскачивание сделано для Windows; на этой ОС установите ffmpeg через пакетный менеджер"
                .to_string(),
        );
    }
    let dest_bin = crate::config::app_data_dir().join("bin");
    let mut last = String::from("источники не заданы");
    for url in URLS {
        eprintln!("[FFMPEG] Пробую источник: {url}");
        match try_install(url, &dest_bin) {
            Ok(()) => return None,
            Err(e) => {
                eprintln!("[FFMPEG] Источник не сработал: {e}");
                last = e;
            }
        }
    }
    Some(format!("Все источники недоступны: {last}"))
}

fn try_install(url: &str, dest_bin: &Path) -> Result<(), String> {
    let tmp_zip = std::env::temp_dir().join(format!("mr-ffmpeg-{}.zip", std::process::id()));
    let _ = std::fs::remove_file(&tmp_zip);

    let resp = ureq::get(url)
        .set("User-Agent", "MeetingRecorder/0.1")
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|e| format!("сеть: {e}"))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    set_progress(0, total);

    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&tmp_zip).map_err(|e| format!("файл: {e}"))?;
    let mut buf = [0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("чтение: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("запись: {e}"))?;
        done += n as u64;
        set_progress(done, total);
    }
    drop(file);
    if done < 1000 {
        return Err(format!("архив слишком мал: {done} байт"));
    }

    let tmp_dir = std::env::temp_dir().join(format!("mr-ffmpeg-x-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("папка: {e}"))?;
    extract_zip(&tmp_zip, &tmp_dir)?;
    let (ff, fp) = find_bins(&tmp_dir)
        .ok_or_else(|| "в архиве нет ffmpeg.exe/ffprobe.exe".to_string())?;

    std::fs::create_dir_all(dest_bin).map_err(|e| format!("bin: {e}"))?;
    std::fs::copy(&ff, dest_bin.join("ffmpeg.exe")).map_err(|e| format!("копия ffmpeg: {e}"))?;
    std::fs::copy(&fp, dest_bin.join("ffprobe.exe")).map_err(|e| format!("копия ffprobe: {e}"))?;
    // Проверка делом: бинарник должен запуститься, а не просто существовать
    if !mr_audio::tools::ffmpeg_available_now() {
        return Err("ffmpeg скопирован, но не запускается (битая сборка?)".to_string());
    }

    let _ = std::fs::remove_file(&tmp_zip);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    eprintln!("[FFMPEG] Установлен в {}", dest_bin.display());
    Ok(())
}

/// Распаковка zip: сначала встроенный `tar` (bsdtar), затем PowerShell.
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let ok = std::process::Command::new("tar")
        .arg("-xf")
        .arg(zip)
        .arg("-C")
        .arg(dest)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    let script = format!(
        "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
        zip.display(),
        dest.display()
    );
    let ok = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    Err("не удалось распаковать архив: нет tar.exe и PowerShell".to_string())
}

/// Рекурсивный поиск ffmpeg.exe/ffprobe.exe в распакованной папке
/// (макет архива зависит от сборщика: `bin/...` или `..._build/bin/...`).
fn find_bins(root: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut ff = None;
    let mut fp = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name == "ffmpeg.exe" {
                    ff = Some(p);
                } else if name == "ffprobe.exe" {
                    fp = Some(p);
                }
            }
        }
    }
    Some((ff?, fp?))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Механика установки без сети: zip со вложенной папкой bin (как в реальных
    /// сборках) распаковывается tar-ом, бинарники находятся и копируются в bin.
    #[test]
    fn extract_find_copy_roundtrip() {
        let base = std::env::temp_dir().join(format!("mr-ffmpeg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/ffmpeg-7.1.1-essentials_build/bin");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("ffmpeg.exe"), b"MZ stub").unwrap();
        std::fs::write(src.join("ffprobe.exe"), b"MZ stub").unwrap();
        let zip = base.join("ffmpeg.zip");
        let st = std::process::Command::new("tar")
            .args(["-a", "-cf"])
            .arg(&zip)
            .arg("-C")
            .arg(base.join("src"))
            .arg("ffmpeg-7.1.1-essentials_build")
            .status()
            .expect("tar");
        assert!(st.success(), "tar не смог собрать zip");

        let out = base.join("out");
        std::fs::create_dir_all(&out).unwrap();
        extract_zip(&zip, &out).expect("распаковка");
        let (ff, fp) = find_bins(&out).expect("бинарники найдены во вложенной папке");
        let dest = base.join("bin");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::copy(&ff, dest.join("ffmpeg.exe")).unwrap();
        std::fs::copy(&fp, dest.join("ffprobe.exe")).unwrap();
        assert!(dest.join("ffmpeg.exe").is_file());
        assert!(dest.join("ffprobe.exe").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }
}
