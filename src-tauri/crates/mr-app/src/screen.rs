#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(any(test, target_os = "macos"))]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(any(windows, target_os = "macos"))]
use std::process::{Child, Stdio};
#[cfg(any(windows, target_os = "macos"))]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(any(windows, target_os = "macos"))]
use std::time::Instant;

#[cfg(target_os = "linux")]
const DEST: &str = "org.gnome.Shell.Screencast";
#[cfg(target_os = "linux")]
const PATH: &str = "/org/gnome/Shell/Screencast";
#[cfg(target_os = "linux")]
const IFACE: &str = "org.gnome.Shell.Screencast";

#[cfg(target_os = "linux")]
fn session_connection() -> Result<zbus::blocking::Connection, String> {
    zbus::blocking::Connection::session().map_err(|e| format!("Сессия D-Bus недоступна: {e}"))
}

#[cfg(target_os = "linux")]
fn screencast_proxy(
    conn: &zbus::blocking::Connection,
) -> Result<zbus::blocking::Proxy<'_>, String> {
    zbus::blocking::Proxy::new(conn, DEST, PATH, IFACE)
        .map_err(|e| format!("Служба записи экрана недоступна: {e}"))
}

pub(crate) struct ScreenCaptureSession {
    #[cfg(target_os = "linux")]
    conn: zbus::blocking::Connection,
    video_path: PathBuf,
    #[cfg(any(windows, target_os = "macos"))]
    child: Child,
}

// Linux: служба появляется при входе в систему, пробу достаточно одного раза.
#[cfg(target_os = "linux")]
static SUPPORTED_CACHE: OnceLock<bool> = OnceLock::new();

// Успех кэшируется навсегда, неудача — на 30 с: ffmpeg можно доустановить
// из интерфейса, и переключатель включится без перезапуска.
pub(crate) fn is_screen_capture_supported() -> bool {
    #[cfg(windows)]
    {
        windows_encoder().is_some()
    }
    #[cfg(target_os = "macos")]
    {
        macos_encoder().is_some()
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
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn cached_encoder(
    cache: &'static Mutex<(Option<&'static str>, Option<Instant>)>,
    probe: fn() -> Option<&'static str>,
) -> Option<&'static str> {
    const RETRY_AFTER: Duration = Duration::from_secs(30);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(encoder) = cache.0 {
        return Some(encoder);
    }
    if let Some(t) = cache.1 {
        if t.elapsed() < RETRY_AFTER {
            return None;
        }
    }
    cache.1 = Some(Instant::now());
    let found = probe();
    cache.0 = found;
    found
}

#[cfg(windows)]
fn windows_encoder() -> Option<&'static str> {
    static ENCODER_CACHE: Mutex<(Option<&'static str>, Option<Instant>)> = Mutex::new((None, None));
    cached_encoder(&ENCODER_CACHE, probe_windows_encoder)
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

#[cfg(target_os = "macos")]
fn macos_encoder() -> Option<&'static str> {
    static ENCODER_CACHE: Mutex<(Option<&'static str>, Option<Instant>)> = Mutex::new((None, None));
    cached_encoder(&ENCODER_CACHE, probe_macos_encoder)
}

#[cfg(target_os = "macos")]
fn probe_macos_encoder() -> Option<&'static str> {
    let out = Command::new(mr_audio::tools::ffmpeg_path())
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if text.contains("h264_videotoolbox") {
        Some("h264_videotoolbox")
    } else if text.contains("libx264") {
        Some("libx264")
    } else {
        None
    }
}

// libx264 может отсутствовать, поэтому выбор энкодера повторяем при старте.
fn fallback_encoder() -> &'static str {
    #[cfg(windows)]
    {
        windows_encoder().unwrap_or("libx264")
    }
    #[cfg(target_os = "macos")]
    {
        macos_encoder().unwrap_or("libx264")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        "libx264"
    }
}

#[cfg(any(test, target_os = "macos"))]
fn macos_ffmpeg_args(encoder: &str, path: &Path) -> Vec<OsString> {
    // avfoundation: "Capture screen 0" — имя устройства, ":none" — без аудио
    // (звук пишет mr-audio отдельно).
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostats",
        "-y",
        "-f",
        "avfoundation",
        "-capture_cursor",
        "1",
        "-framerate",
        "30",
        "-i",
        "Capture screen 0:none",
        "-c:v",
        encoder,
    ]
    .iter()
    .map(OsString::from)
    .collect();
    if encoder == "h264_videotoolbox" {
        args.extend(["-b:v", "8M"].iter().map(OsString::from));
    } else if encoder == "libx264" {
        args.extend(
            ["-preset", "ultrafast", "-crf", "25"]
                .iter()
                .map(OsString::from),
        );
    }
    args.extend(
        ["-pix_fmt", "yuv420p", "-movflags", "+faststart"]
            .iter()
            .map(OsString::from),
    );
    args.push(path.into());
    args
}

#[cfg(any(windows, target_os = "macos"))]
fn spawn_child(cmd: &mut Command, video_path: &Path) -> Result<Child, String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        // stderr не читаем во время записи: заполненный pipe заблокировал бы
        // ffmpeg. Диагностика старта ниже и так проверяет только код выхода.
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Не удалось запустить ffmpeg: {e}"))?;
    // ffmpeg не печатает READY, поэтому handshake — процесс жив спустя секунду
    std::thread::sleep(Duration::from_millis(1200));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("ffmpeg завершился сразу ({status})"));
    }
    crate::note!("[SCREEN] Запись экрана запущена: {video_path:?}");

    Ok(child)
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
        cmd.arg("-pix_fmt").arg("yuv420p").arg(&video_path);

        Ok(Self {
            child: spawn_child(&mut cmd, &video_path)?,
            video_path,
        })
    }

    // shortcut: ffmpeg avfoundation вместо SCK+AVAssetWriter (нет Mac для проверки TCC), заменить после живой проверки.
    #[cfg(target_os = "macos")]
    pub(crate) fn start(started_ts: u64) -> Result<Self, String> {
        let Some(encoder) = macos_encoder() else {
            return Err("ffmpeg с кодировщиком H.264 не найден в PATH".to_string());
        };
        let video_path = std::env::temp_dir().join(format!(".mr_screen_{started_ts}.mp4"));

        let mut cmd = Command::new(mr_audio::tools::ffmpeg_path());
        cmd.args(macos_ffmpeg_args(encoder, &video_path));
        // ffmpeg без разрешения TCC выходит сразу — подсказываем, что включить.
        let child = spawn_child(&mut cmd, &video_path).map_err(|e| {
            format!(
                "{e}. Разрешите «Запись экрана и системного аудио» в «Системные настройки → Конфиденциальность и безопасность»"
            )
        })?;

        Ok(Self { child, video_path })
    }

    /// Запускает запись экрана (Linux): служба GNOME Shell Screencast по D-Bus.
    #[cfg(target_os = "linux")]
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
        crate::note!("[SCREEN] Запись экрана запущена: {video_path:?}");

        Ok(Self { conn, video_path })
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    pub(crate) fn start(_started_ts: u64) -> Result<Self, String> {
        Err("Запись экрана не поддерживается на этой платформе".to_string())
    }

    // start здесь всегда возвращает ошибку, но вызывающий код компилирует stop
    // на всех платформах: без заглушки Android и iOS не собираются.
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    pub(crate) fn stop(self) -> Result<PathBuf, String> {
        Err("Запись экрана не поддерживается на этой платформе".to_string())
    }

    // ffmpeg выходит по 'q' из stdin — аналога EOF-сигнала хелпера Linux.
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn stop(mut self) -> Result<PathBuf, String> {
        crate::note!("[SCREEN] Остановка записи экрана...");
        {
            use std::io::Write;
            if let Some(mut si) = self.child.stdin.take() {
                let _ = si.write_all(b"q");
            }
        }

        let start_wait = Instant::now();
        let mut exited = false;
        while start_wait.elapsed() < Duration::from_secs(7) {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    crate::note!("[SCREEN] Процесс захвата завершился со статусом {status:?}");
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => {
                    crate::note!("[SCREEN] Ошибка ожидания процесса захвата: {e}");
                    break;
                }
            }
        }

        if !exited {
            crate::note!("[SCREEN] Процесс захвата не завершился вовремя, отправляю kill");
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        wait_finalized(&self.video_path)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn stop(self) -> Result<PathBuf, String> {
        let proxy = screencast_proxy(&self.conn)?;
        proxy
            .call::<_, _, bool>("StopScreencast", &())
            .map_err(|e| format!("Не удалось остановить запись экрана: {e}"))?;
        crate::note!("[SCREEN] Остановка записи экрана...");
        wait_finalized(&self.video_path)
    }
}

/// Ждёт, пока служба/процесс закроет видеофайл (подстраховка на случай лагов).
fn wait_finalized(video_path: &Path) -> Result<PathBuf, String> {
    for _ in 0..30 {
        if video_path.is_file() {
            let size = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            if size > 1000 {
                crate::note!("[SCREEN] Записано видео: {video_path:?} ({size} байт)");
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
        // P7: сессия без явного stop() не оставляет захват писать видео.
        #[cfg(any(windows, target_os = "macos"))]
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        #[cfg(target_os = "linux")]
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
    crate::note!("[SCREEN] Склейка видео {video_path:?} и аудио {audio_path:?} -> {final_path:?}");

    let remove_leftovers = || {
        let _ = std::fs::remove_file(video_path);
        let _ = std::fs::remove_file(final_path);
    };

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

    let output = match cmd.output() {
        Ok(output) => output,
        Err(e) => {
            remove_leftovers();
            return Err(format!("Не удалось запустить ffmpeg: {e}"));
        }
    };
    if output.status.success() && final_path.is_file() {
        let size = std::fs::metadata(final_path).map(|m| m.len()).unwrap_or(0);
        if size > 1000 {
            crate::note!("[SCREEN] Склейка успешна (-c:v copy): {size} байт");
            let _ = std::fs::remove_file(video_path);
            let _ = std::fs::remove_file(audio_path);
            return Ok(());
        }
    }

    crate::note!("[SCREEN] Прямое копирование не сработало, запускаю перекодирование...");
    let encoder = fallback_encoder();
    let mut cmd = Command::new(mr_audio::tools::ffmpeg_path());
    cmd.arg("-y")
        .arg("-i")
        .arg(video_path)
        .arg("-i")
        .arg(audio_path)
        .arg("-c:v")
        .arg(encoder);
    if encoder == "libx264" {
        cmd.args(["-preset", "ultrafast", "-crf", "23"]);
    }
    cmd.arg("-c:a").arg("aac").arg("-shortest").arg(final_path);

    let output = match cmd.output() {
        Ok(output) => output,
        Err(e) => {
            remove_leftovers();
            return Err(format!("Не удалось запустить ffmpeg: {e}"));
        }
    };
    if output.status.success() && final_path.is_file() {
        let size = std::fs::metadata(final_path).map(|m| m.len()).unwrap_or(0);
        if size > 1000 {
            crate::note!("[SCREEN] Перекодирование успешно: {size} байт");
            let _ = std::fs::remove_file(video_path);
            let _ = std::fs::remove_file(audio_path);
            return Ok(());
        }
    }

    // Неудачная склейка не должна оставлять мусор: исходник экрана и частичный mp4.
    remove_leftovers();
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

    #[test]
    fn macos_ffmpeg_args_capture_screen_input() {
        let path = Path::new("/tmp/.mr_screen_1.mp4");
        let to_strings = |encoder: &str| -> Vec<String> {
            macos_ffmpeg_args(encoder, path)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };

        let args = to_strings("h264_videotoolbox");
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-f" && w[1] == "avfoundation"));
        assert!(args.iter().any(|a| a == "Capture screen 0:none"));
        assert!(args.iter().any(|a| a == "h264_videotoolbox"));
        assert!(args.windows(2).any(|w| w[0] == "-b:v" && w[1] == "8M"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "-pix_fmt" && w[1] == "yuv420p"));
        assert_eq!(
            args.last().map(String::as_str),
            Some("/tmp/.mr_screen_1.mp4")
        );

        let x264 = to_strings("libx264");
        assert!(x264
            .windows(2)
            .any(|w| w[0] == "-preset" && w[1] == "ultrafast"));
        assert!(x264.windows(2).any(|w| w[0] == "-crf" && w[1] == "25"));
        assert!(!x264.iter().any(|a| a == "8M"));
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
            crate::note!("ffmpeg недоступен — пропуск живого теста");
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
