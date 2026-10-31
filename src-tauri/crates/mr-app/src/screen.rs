//! Захват экрана на Linux (GNOME Shell Screencast D-Bus).
//! Управление записью - нативный D-Bus-клиент (zbus), внешних интерпретаторов нет.
//! Склейка видеофайла экрана с аудиозаписью встречи - через ffmpeg.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

const DEST: &str = "org.gnome.Shell.Screencast";
const PATH: &str = "/org/gnome/Shell/Screencast";
const IFACE: &str = "org.gnome.Shell.Screencast";

fn session_connection() -> Result<zbus::blocking::Connection, String> {
    zbus::blocking::Connection::session().map_err(|e| format!("Сессия D-Bus недоступна: {e}"))
}

fn screencast_proxy(
    conn: &zbus::blocking::Connection,
) -> Result<zbus::blocking::Proxy<'_>, String> {
    zbus::blocking::Proxy::new(conn, DEST, PATH, IFACE)
        .map_err(|e| format!("Служба записи экрана недоступна: {e}"))
}

pub(crate) struct ScreenCaptureSession {
    conn: zbus::blocking::Connection,
    video_path: PathBuf,
}

static SUPPORTED_CACHE: OnceLock<bool> = OnceLock::new();

/// Проверяет, доступна ли служба захвата экрана на текущей системе.
pub(crate) fn is_screen_capture_supported() -> bool {
    *SUPPORTED_CACHE.get_or_init(|| {
        if !cfg!(target_os = "linux") {
            return false;
        }
        session_connection()
            .and_then(|conn| {
                let supported: bool = screencast_proxy(&conn)?
                    .get_property("ScreencastSupported")
                    .unwrap_or(false);
                Ok(supported)
            })
            .unwrap_or(false)
    })
}

impl ScreenCaptureSession {
    /// Запускает запись экрана. Временный файл пишется в системный temp.
    pub(crate) fn start(started_ts: u64) -> Result<Self, String> {
        let conn = session_connection()?;
        let template = format!("/tmp/.mr_screen_{started_ts}_%d_%t");
        let options: HashMap<&str, zbus::zvariant::Value> =
            HashMap::from([("draw-cursor", zbus::zvariant::Value::from(true))]);
        let (success, filename): (bool, String) = {
            let proxy = screencast_proxy(&conn)?;
            proxy
                .call("Screencast", &(&template, &options))
                .map_err(|e| format!("Служба записи экрана не запустилась: {e}"))?
        };
        if !success {
            return Err("GNOME Shell не начал запись экрана".to_string());
        }
        let video_path = PathBuf::from(filename);
        eprintln!("[SCREEN] Запись экрана запущена: {video_path:?}");

        Ok(Self { conn, video_path })
    }

    /// Останавливает запись экрана и ожидает финализации видеофайла.
    pub(crate) fn stop(self) -> Result<PathBuf, String> {
        let proxy = screencast_proxy(&self.conn)?;
        proxy
            .call::<_, _, bool>("StopScreencast", &())
            .map_err(|e| format!("Не удалось остановить запись экрана: {e}"))?;
        eprintln!("[SCREEN] Остановка записи экрана...");

        // Файл закрывается службой сразу после ответа; подстраховка на случай лагов.
        let video_path = self.video_path.clone();
        for _ in 0..30 {
            if video_path.is_file() {
                let size = std::fs::metadata(&video_path).map(|m| m.len()).unwrap_or(0);
                if size > 1000 {
                    eprintln!("[SCREEN] Записано видео: {video_path:?} ({size} байт)");
                    return Ok(video_path);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        if video_path.is_file() {
            let size = std::fs::metadata(&video_path).map(|m| m.len()).unwrap_or(0);
            if size > 0 {
                return Ok(video_path);
            }
        }

        Err(format!("Файл видео не найден или пуст: {video_path:?}"))
    }
}

impl Drop for ScreenCaptureSession {
    fn drop(&mut self) {
        // P7: сессия без явного stop() не оставляет GNOME Shell писать видео.
        if let Ok(proxy) = screencast_proxy(&self.conn) {
            let _ = proxy.call::<_, _, bool>("StopScreencast", &());
        }
    }
}

/// Склеивает видеофайл экрана и аудиозапись встречи через ffmpeg.
pub(crate) fn mux_screen_and_audio(
    video_path: &Path,
    audio_path: &Path,
    final_path: &Path,
) -> Result<(), String> {
    eprintln!("[SCREEN] Склейка видео {video_path:?} и аудио {audio_path:?} -> {final_path:?}");

    // 1-я попытка: быстрое копирование видеопотока h264 + кодирование aac
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i")
        .arg(video_path)
        .arg("-i")
        .arg(audio_path)
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("aac")
        .arg("-shortest")
        .arg(final_path);

    let output = cmd
        .output()
        .map_err(|e| format!("Не удалось запустить ffmpeg: {e}"))?;
    if output.status.success() && final_path.is_file() {
        let size = std::fs::metadata(final_path).map(|m| m.len()).unwrap_or(0);
        if size > 1000 {
            eprintln!("[SCREEN] Склейка успешна (-c:v copy): {size} байт");
            let _ = std::fs::remove_file(video_path);
            let _ = std::fs::remove_file(audio_path);
            return Ok(());
        }
    }

    // 2-я попытка: перекодирование, если прямое копирование контейнера не сработало
    eprintln!("[SCREEN] Прямое копирование не сработало, запускаю перекодирование...");
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i")
        .arg(video_path)
        .arg("-i")
        .arg(audio_path)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("ultrafast")
        .arg("-crf")
        .arg("23")
        .arg("-c:a")
        .arg("aac")
        .arg("-shortest")
        .arg(final_path);

    let output = cmd
        .output()
        .map_err(|e| format!("Не удалось запустить ffmpeg: {e}"))?;
    if output.status.success() && final_path.is_file() {
        let size = std::fs::metadata(final_path).map(|m| m.len()).unwrap_or(0);
        if size > 1000 {
            eprintln!("[SCREEN] Перекодирование успешно: {size} байт");
            let _ = std::fs::remove_file(video_path);
            let _ = std::fs::remove_file(audio_path);
            return Ok(());
        }
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "Ошибка ffmpeg: {}",
        stderr.chars().take(300).collect::<String>()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_support_check() {
        let _ = is_screen_capture_supported();
    }
}
