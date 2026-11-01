#[cfg(not(windows))]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::{Child, Stdio};
#[cfg(windows)]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

#[cfg(not(windows))]
const DEST: &str = "org.gnome.Shell.Screencast";
#[cfg(not(windows))]
const PATH: &str = "/org/gnome/Shell/Screencast";
#[cfg(not(windows))]
const IFACE: &str = "org.gnome.Shell.Screencast";

#[cfg(not(windows))]
fn session_connection() -> Result<zbus::blocking::Connection, String> {
    zbus::blocking::Connection::session().map_err(|e| format!("Сессия D-Bus недоступна: {e}"))
}

#[cfg(not(windows))]
fn screencast_proxy(
    conn: &zbus::blocking::Connection,
) -> Result<zbus::blocking::Proxy<'_>, String> {
    zbus::blocking::Proxy::new(conn, DEST, PATH, IFACE)
        .map_err(|e| format!("Служба записи экрана недоступна: {e}"))
}

pub(crate) struct ScreenCaptureSession {
    #[cfg(not(windows))]
    conn: zbus::blocking::Connection,
    video_path: PathBuf,
    #[cfg(windows)]
    child: Child,
}

// Linux: служба появляется при входе в систему, пробу достаточно одного раза.
#[cfg(target_os = "linux")]
static SUPPORTED_CACHE: OnceLock<bool> = OnceLock::new();

/// На Windows поддержка зависит от ffmpeg, который можно доустановить
/// из интерфейса: успех кэшируется навсегда, неудача перепроверяется
/// не чаще раза в 30 с — переключатель включается без перезапуска.
pub(crate) fn is_screen_capture_supported() -> bool {
    #[cfg(windows)]
    {
        windows_encoder().is_some()
    }
    #[cfg(target_os = "linux")]
    {
        *SUPPORTED_CACHE.get_or_init(|| {
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
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

#[cfg(windows)]
fn windows_encoder() -> Option<&'static str> {
    static ENCODER_CACHE: Mutex<(Option<&'static str>, Option<Instant>)> = Mutex::new((None, None));
    const RETRY_AFTER: Duration = Duration::from_secs(30);
    let mut cache = ENCODER_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(encoder) = cache.0 {
        return Some(encoder);
    }
    if let Some(t) = cache.1 {
        if t.elapsed() < RETRY_AFTER {
            return None;
        }
    }
    cache.1 = Some(Instant::now());
    let found = probe_windows_encoder();
    cache.0 = found;
    found
}

#[cfg(windows)]
fn probe_windows_encoder() -> Option<&'static str> {
    let out = Command::new(mr_audio::tools::ffmpeg_path())
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if text.contains("libx264") {
        Some("libx264")
    } else if text.contains("h264_mf") {
        Some("h264_mf")
    } else {
        None
    }
}

impl ScreenCaptureSession {
    #[cfg(windows)]
    pub(crate) fn start(started_ts: u64) -> Result<Self, String> {
        let Some(encoder) = windows_encoder() else {
            return Err("ffmpeg с кодировщиком H.264 не найден в PATH".to_string());
        };
        let video_path = std::env::temp_dir().join(format!(".mr_screen_{started_ts}.mp4"));

        let mut cmd = Command::new(mr_audio::tools::ffmpeg_path());
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostats",
            "-y",
            "-f",
            "gdigrab",
            "-framerate",
            "15",
            "-i",
            "desktop",
            "-c:v",
            encoder,
        ]);
        if encoder == "libx264" {
            cmd.args(["-preset", "ultrafast", "-crf", "25"]);
        }
        cmd.arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&video_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Не удалось запустить ffmpeg: {e}"))?;
        // ffmpeg не печатает READY, поэтому handshake — процесс жив спустя секунду
        std::thread::sleep(Duration::from_millis(1200));
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut err);
            }
            return Err(format!(
                "ffmpeg завершился сразу ({status}): {}",
                err.trim()
            ));
        }
        eprintln!("[SCREEN] Запись экрана запущена: {video_path:?}");

        Ok(Self { child, video_path })
    }

    /// Запускает запись экрана (Linux): служба GNOME Shell Screencast по D-Bus.
    #[cfg(not(windows))]
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

    // ffmpeg выходит по 'q' из stdin — аналога EOF-сигнала хелпера Linux.
    #[cfg(windows)]
    pub(crate) fn stop(mut self) -> Result<PathBuf, String> {
        eprintln!("[SCREEN] Остановка записи экрана...");
        {
            use std::io::Write;
            if let Some(mut si) = self.child.stdin.take() {
                let _ = si.write_all(b"q");
            }
        }

        let start_wait = std::time::Instant::now();
        let mut exited = false;
        while start_wait.elapsed() < Duration::from_secs(7) {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!("[SCREEN] Процесс захвата завершился со статусом {status:?}");
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => {
                    eprintln!("[SCREEN] Ошибка ожидания процесса захвата: {e}");
                    break;
                }
            }
        }

        if !exited {
            eprintln!("[SCREEN] Процесс захвата не завершился вовремя, отправляю kill");
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        wait_finalized(&self.video_path)
    }

    #[cfg(not(windows))]
    pub(crate) fn stop(self) -> Result<PathBuf, String> {
        let proxy = screencast_proxy(&self.conn)?;
        proxy
            .call::<_, _, bool>("StopScreencast", &())
            .map_err(|e| format!("Не удалось остановить запись экрана: {e}"))?;
        eprintln!("[SCREEN] Остановка записи экрана...");
        wait_finalized(&self.video_path)
    }
}

/// Ждёт, пока служба/процесс закроет видеофайл (подстраховка на случай лагов).
fn wait_finalized(video_path: &Path) -> Result<PathBuf, String> {
    for _ in 0..30 {
        if video_path.is_file() {
            let size = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            if size > 1000 {
                eprintln!("[SCREEN] Записано видео: {video_path:?} ({size} байт)");
                return Ok(video_path.to_path_buf());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if video_path.is_file() {
        let size = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
        if size > 0 {
            return Ok(video_path.to_path_buf());
        }
    }

    Err(format!("Файл видео не найден или пуст: {video_path:?}"))
}

impl Drop for ScreenCaptureSession {
    fn drop(&mut self) {
        // P7: сессия без явного stop() не оставляет службу писать видео.
        #[cfg(windows)]
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        #[cfg(not(windows))]
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

    let mut cmd = Command::new(mr_audio::tools::ffmpeg_path());
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

    eprintln!("[SCREEN] Прямое копирование не сработало, запускаю перекодирование...");
    let mut cmd = Command::new(mr_audio::tools::ffmpeg_path());
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

/// Живой тест Windows: реальный захват экрана ~2 секунды (нужен ffmpeg в PATH,
/// иначе тест пропускается — CI-раннеры без ffmpeg не падают).
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn screen_capture_roundtrip_writes_valid_video() {
        if !is_screen_capture_supported() {
            eprintln!("ffmpeg недоступен — пропуск живого теста");
            return;
        }
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let session = ScreenCaptureSession::start(started).expect("старт записи экрана");
        std::thread::sleep(Duration::from_secs(2));
        let video = session.stop().expect("остановка записи экрана");
        let size = std::fs::metadata(&video)
            .expect("файл видео существует")
            .len();
        assert!(size > 1000, "видео слишком маленькое: {size} байт");
        // Файл должен быть валидным контейнером mp4, а не просто непустым
        if let Ok(out) = Command::new(mr_audio::tools::ffprobe_path())
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
            ])
            .arg(&video)
            .output()
        {
            assert!(out.status.success(), "ffprobe не прочитал видео {video:?}");
        }
        let _ = std::fs::remove_file(&video);
    }
}
