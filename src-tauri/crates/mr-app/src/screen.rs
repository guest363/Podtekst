#[cfg(target_os = "linux")]
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(any(windows, target_os = "macos"))]
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

#[cfg(target_os = "linux")]
const DEST: &str = "org.gnome.Shell.Screencast";
#[cfg(target_os = "linux")]
const PATH: &str = "/org/gnome/Shell/Screencast";
#[cfg(target_os = "linux")]
const IFACE: &str = "org.gnome.Shell.Screencast";

#[cfg(target_os = "linux")]
async fn session_connection() -> Result<zbus::Connection, String> {
    zbus::Connection::session()
        .await
        .map_err(|e| format!("Сессия D-Bus недоступна: {e}"))
}

#[cfg(target_os = "linux")]
async fn screencast_proxy<'a>(conn: &'a zbus::Connection) -> Result<zbus::Proxy<'a>, String> {
    zbus::Proxy::new(conn, DEST, PATH, IFACE)
        .await
        .map_err(|e| format!("Служба записи экрана недоступна: {e}"))
}

pub(crate) struct ScreenCaptureSession {
    #[cfg(target_os = "linux")]
    conn: zbus::Connection,
    video_path: PathBuf,
    /// Насколько позже аудио началось видео: расхождение компенсируется при склейке.
    offset_sec: f64,
    #[cfg(any(windows, target_os = "macos"))]
    child: Child,
}

impl ScreenCaptureSession {
    pub(crate) fn offset_sec(&self) -> f64 {
        self.offset_sec
    }
}

fn video_offset(audio_started: Instant) -> f64 {
    Instant::now().duration_since(audio_started).as_secs_f64()
}

// Успех кэшируется навсегда, неудача — на 30 с: служба записи появляется при
// входе в GNOME-сессию, и переключатель включается без перезапуска приложения.
#[cfg(target_os = "linux")]
static SUPPORTED_CACHE: Mutex<(Option<bool>, Option<Instant>)> = Mutex::new((None, None));

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
        cached_flag(
            &SUPPORTED_CACHE,
            Duration::from_secs(30),
            probe_screencast_supported,
        )
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn probe_screencast_supported() -> bool {
    zbus::block_on(async {
        let conn = session_connection().await.ok()?;
        let proxy = screencast_proxy(&conn).await.ok()?;
        proxy.get_property("ScreencastSupported").await.ok()
    })
    .unwrap_or(false)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cached_flag(
    cache: &Mutex<(Option<bool>, Option<Instant>)>,
    retry_after: Duration,
    probe: fn() -> bool,
) -> bool {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.0 == Some(true) {
        return true;
    }
    if let Some(at) = cache.1 {
        if at.elapsed() < retry_after {
            return false;
        }
    }
    cache.1 = Some(Instant::now());
    let found = probe();
    cache.0 = Some(found);
    found
}

#[cfg(any(windows, target_os = "macos"))]
static ENCODER_CACHE: Mutex<(Option<&'static str>, Option<Instant>)> = Mutex::new((None, None));

/// Сброс после установки ffmpeg: старый выбранный энкодер мог исчезнуть.
pub(crate) fn invalidate_encoders() {
    #[cfg(any(windows, target_os = "macos"))]
    if let Ok(mut cache) = ENCODER_CACHE.lock() {
        *cache = (None, None);
    }
    #[cfg(target_os = "linux")]
    {
        *SUPPORTED_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = (None, None);
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
    crate::note!(
        "[SCREEN] Запись экрана запущена: {}",
        crate::logging::file_token(video_path)
    );

    Ok(child)
}

impl ScreenCaptureSession {
    #[cfg(windows)]
    pub(crate) fn start(started_ts: u64, audio_started: Instant) -> Result<Self, String> {
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

        let offset_sec = video_offset(audio_started);
        Ok(Self {
            child: spawn_child(&mut cmd, &video_path)?,
            video_path,
            offset_sec,
        })
    }

    // shortcut: ffmpeg avfoundation вместо SCK+AVAssetWriter (нет Mac для проверки TCC), заменить после живой проверки.
    #[cfg(target_os = "macos")]
    pub(crate) fn start(started_ts: u64, audio_started: Instant) -> Result<Self, String> {
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

        Ok(Self {
            child,
            video_path,
            offset_sec: video_offset(audio_started),
        })
    }

    /// Запускает запись экрана (Linux): служба GNOME Shell Screencast по D-Bus.
    #[cfg(target_os = "linux")]
    pub(crate) fn start(started_ts: u64, audio_started: Instant) -> Result<Self, String> {
        let (conn, filename) = zbus::block_on(async {
            let conn = session_connection().await?;
            let template = format!("/tmp/.mr_screen_{started_ts}_%d_%t");
            let options: HashMap<&str, zbus::Variant> =
                HashMap::from([("draw-cursor", zbus::Variant::from(true))]);
            let proxy = screencast_proxy(&conn).await?;
            let (success, filename): (bool, String) = proxy
                .call("Screencast", &(&template, &options))
                .await
                .map_err(|e| format!("Служба записи экрана не запустилась: {e}"))?;
            if !success {
                return Err("GNOME Shell не начал запись экрана".to_string());
            }
            Ok::<_, String>((conn, filename))
        })?;
        let video_path = PathBuf::from(filename);
        crate::note!(
            "[SCREEN] Запись экрана запущена: {}",
            crate::logging::file_token(&video_path)
        );

        Ok(Self {
            conn,
            video_path,
            offset_sec: video_offset(audio_started),
        })
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    pub(crate) fn start(_started_ts: u64, _audio_started: Instant) -> Result<Self, String> {
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
        zbus::block_on(async {
            let proxy = screencast_proxy(&self.conn).await?;
            proxy
                .call::<_, _, bool>("StopScreencast", &())
                .await
                .map_err(|e| format!("Не удалось остановить запись экрана: {e}"))?;
            Ok::<_, String>(())
        })?;
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
                crate::note!(
                    "[SCREEN] Записано видео: {} ({size} байт)",
                    crate::logging::file_token(video_path)
                );
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
        let _ = zbus::block_on(async {
            let proxy = screencast_proxy(&self.conn).await.ok()?;
            let _: bool = proxy.call("StopScreencast", &()).await.ok()?;
            Some(())
        });
    }
}

/// Аргументы до первого `-i`: видео стартовало позже аудио, поэтому его
/// шкалу сдвигаем на измеренное расхождение (`-itsoffset`).
fn mux_args(video_offset_sec: f64) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-y", "-itsoffset"]
        .iter()
        .map(|arg| OsString::from(*arg))
        .collect();
    args.push(OsString::from(format!("{video_offset_sec:.3}")));
    args
}

/// Склеивает видеофайл экрана и аудиозапись встречи через ffmpeg.
pub(crate) fn mux_screen_and_audio(
    video_path: &Path,
    audio_path: &Path,
    final_path: &Path,
    video_offset_sec: f64,
) -> Result<(), String> {
    crate::note!(
        "[SCREEN] Склейка видео {} и аудио {} -> {}",
        crate::logging::file_token(video_path),
        crate::logging::file_token(audio_path),
        crate::logging::file_token(final_path)
    );

    // Исходник экрана — единственная копия видео: сбой склейки не должен его удалять.
    let fail = |reason: String| -> Result<(), String> {
        let _ = std::fs::remove_file(final_path);
        let kept = keep_raw_video(video_path, audio_path);
        let note = kept
            .map(|path| {
                format!(
                    "; запись экрана сохранена: {}",
                    crate::logging::file_token(&path)
                )
            })
            .unwrap_or_default();
        Err(format!("{reason}{note}"))
    };

    let mut cmd = Command::new(mr_audio::tools::ffmpeg_path());
    cmd.args(mux_args(video_offset_sec));
    cmd.arg("-i")
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
        Err(e) => return fail(format!("Не удалось запустить ffmpeg: {e}")),
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
    cmd.args(mux_args(video_offset_sec));
    cmd.arg("-i")
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
        Err(e) => return fail(format!("Не удалось запустить ffmpeg: {e}")),
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    fail(format!(
        "Ошибка ffmpeg: {}",
        stderr.chars().take(300).collect::<String>()
    ))
}

/// Кладёт сырое видео экрана рядом с аудио, чтобы сбой склейки не уничтожил запись.
fn keep_raw_video(video_path: &Path, audio_path: &Path) -> Option<PathBuf> {
    let dir = audio_path.parent()?;
    let stem = audio_path.file_stem()?.to_string_lossy();
    let ext = video_path
        .extension()
        .map(|ext| ext.to_string_lossy().to_string())
        .unwrap_or_else(|| "mp4".to_string());
    let dest = crate::recording::unique_media_path(dir, &format!("{stem}.screen.{ext}"));
    std::fs::rename(video_path, &dest).ok().map(|()| dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_support_is_checked_without_panic() {
        // Значение зависит от платформы; важен инвариант: повторный вызов не падает
        // и не переспрашивает службу чаще, чем разрешает кэш.
        let _ = is_screen_capture_supported();
        let _ = is_screen_capture_supported();
    }

    #[test]
    fn failed_support_probe_is_retried_after_cache_window() {
        static CACHE: Mutex<(Option<bool>, Option<Instant>)> = Mutex::new((None, None));
        static FAILS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        fn failing() -> bool {
            FAILS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        }
        assert!(
            !cached_flag(&CACHE, Duration::from_secs(30), failing),
            "первая проба"
        );
        assert!(
            !cached_flag(&CACHE, Duration::from_secs(30), failing),
            "в окне кэша"
        );
        assert_eq!(
            FAILS.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "отрицательный результат не должен переспрашиваться каждую секунду"
        );
        assert!(
            !cached_flag(&CACHE, Duration::ZERO, failing),
            "после окна проба повторяется"
        );
        assert_eq!(FAILS.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn mux_shifts_shifted_video_by_measured_offset() {
        let args: Vec<String> = mux_args(1.734)
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(args, vec!["-y", "-itsoffset", "1.734"]);
    }

    #[test]
    fn failed_mux_keeps_raw_screen_video_next_to_audio() {
        let dir = std::env::temp_dir().join(format!("mr-screen-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("meeting.wav");
        std::fs::write(&audio, b"wav").unwrap();
        let video = dir.join(".mr_screen_1.mp4");
        std::fs::write(&video, b"video").unwrap();

        let kept = keep_raw_video(&video, &audio).expect("видео обязано сохраниться");
        assert_eq!(
            kept.file_name().map(|n| n.to_string_lossy().to_string()),
            Some("meeting.screen.mp4".to_string())
        );
        assert!(kept.is_file());
        assert!(!video.exists(), "сырой файл переехал, а не остался в temp");

        let _ = std::fs::remove_dir_all(&dir);
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
        let session = ScreenCaptureSession::start(started, std::time::Instant::now())
            .expect("старт записи экрана");
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
