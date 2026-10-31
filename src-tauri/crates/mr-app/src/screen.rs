//! Захват экрана: Linux (GNOME Shell Screencast D-Bus) и Windows (ffmpeg gdigrab).
//! Управляет записью экрана в отдельном процессе и склейкой с аудио через ffmpeg.

#[cfg(not(windows))]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

#[cfg(not(windows))]
const HELPER_SCRIPT: &str = r#"
import sys, signal, time

try:
    import dbus
    bus = dbus.SessionBus()
    obj = bus.get_object('org.gnome.Shell.Screencast', '/org/gnome/Shell/Screencast')
    iface = dbus.Interface(obj, 'org.gnome.Shell.Screencast')
except Exception as e:
    sys.stderr.write(f"INIT_ERR:{e}\n")
    sys.stderr.flush()
    sys.exit(2)

def on_stop(*args):
    try:
        iface.StopScreencast()
    except Exception:
        pass
    sys.exit(0)

signal.signal(signal.SIGTERM, on_stop)
signal.signal(signal.SIGINT, on_stop)

template = sys.argv[1]
try:
    success, filename = iface.Screencast(template, {'draw-cursor': dbus.Boolean(True)})
    if not success:
        sys.stderr.write("SCREENCAST_FAILED\n")
        sys.stderr.flush()
        sys.exit(1)
    sys.stdout.write(f"READY:{filename}\n")
    sys.stdout.flush()
except Exception as e:
    sys.stderr.write(f"SCREENCAST_ERR:{e}\n")
    sys.stderr.flush()
    sys.exit(1)

try:
    # Ожидаем закрытия stdin от родительского процесса
    sys.stdin.read()
except Exception:
    pass
on_stop()
"#;

pub(crate) struct ScreenCaptureSession {
    child: Child,
    video_path: PathBuf,
}

static SUPPORTED_CACHE: OnceLock<bool> = OnceLock::new();

/// Проверяет, доступна ли служба захвата экрана на текущей системе.
pub(crate) fn is_screen_capture_supported() -> bool {
    *SUPPORTED_CACHE.get_or_init(platform_support_probe)
}

/// Проба доступности захвата для конкретной ОС (python3 + служба GNOME).
#[cfg(target_os = "linux")]
fn platform_support_probe() -> bool {
    let check_script = r#"
import sys
try:
    import dbus
    bus = dbus.SessionBus()
    obj = bus.get_object('org.gnome.Shell.Screencast', '/org/gnome/Shell/Screencast')
    sys.exit(0)
except Exception:
    sys.exit(1)
"#;
    Command::new("python3")
        .args(["-c", check_script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Проба доступности захвата для конкретной ОС: нужен ffmpeg с кодировщиком H.264.
#[cfg(windows)]
fn platform_support_probe() -> bool {
    windows_encoder().is_some()
}

/// Заглушка для прочих платформ (macOS и др.): захват экрана не реализован.
#[cfg(not(any(target_os = "linux", windows)))]
fn platform_support_probe() -> bool {
    false
}

/// Кодировщик H.264 из установленного ffmpeg: libx264, иначе аппаратный h264_mf.
#[cfg(windows)]
fn windows_encoder() -> Option<&'static str> {
    static ENCODER_CACHE: OnceLock<Option<&'static str>> = OnceLock::new();
    *ENCODER_CACHE.get_or_init(|| {
        let out = Command::new("ffmpeg")
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
    })
}

impl ScreenCaptureSession {
    /// Запускает запись экрана (Windows): ffmpeg gdigrab пишет весь рабочий стол в mp4.
    #[cfg(windows)]
    pub(crate) fn start(started_ts: u64) -> Result<Self, String> {
        let Some(encoder) = windows_encoder() else {
            return Err("ffmpeg с кодировщиком H.264 не найден в PATH".to_string());
        };
        let video_path = std::env::temp_dir().join(format!(".mr_screen_{started_ts}.mp4"));

        let mut cmd = Command::new("ffmpeg");
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

    /// Запускает запись экрана (Linux/macOS): python3-хелпер GNOME Shell Screencast.
    /// Временный файл пишется в системный temp.
    #[cfg(not(windows))]
    pub(crate) fn start(started_ts: u64) -> Result<Self, String> {
        let template = format!("/tmp/.mr_screen_{started_ts}_%d_%t");
        let mut cmd = Command::new("python3");
        cmd.args(["-c", HELPER_SCRIPT, &template])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Не удалось запустить python3: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Не удалось открыть stdout процесса захвата".to_string())?;

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Ошибка чтения ответа захвата: {e}"))?;

        let line = line.trim();
        if !line.starts_with("READY:") {
            let mut err = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut err);
            }
            let _ = child.kill();
            return Err(format!(
                "Служба записи экрана не запустилась: {} {}",
                line,
                err.trim()
            ));
        }

        let video_path = PathBuf::from(line.trim_start_matches("READY:").trim());
        eprintln!("[SCREEN] Запись экрана запущена: {video_path:?}");

        Ok(Self { child, video_path })
    }

    /// Останавливает запись экрана и ожидает финализации видеофайла.
    pub(crate) fn stop(mut self) -> Result<PathBuf, String> {
        eprintln!("[SCREEN] Остановка записи экрана...");
        // Сигнал остановки: EOF в stdin хелпера (Linux) или клавиша 'q' в ffmpeg (Windows)
        #[cfg(windows)]
        {
            use std::io::Write;
            if let Some(mut si) = self.child.stdin.take() {
                let _ = si.write_all(b"q");
            }
        }
        #[cfg(not(windows))]
        drop(self.child.stdin.take());

        // Ждём завершения процесса до 7 секунд
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

        // Ждем пока файл появится и закроется
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
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        let size = std::fs::metadata(&video).expect("файл видео существует").len();
        assert!(size > 1000, "видео слишком маленькое: {size} байт");
        // Файл должен быть валидным контейнером mp4, а не просто непустым
        if let Ok(out) = Command::new("ffprobe")
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
            assert!(
                out.status.success(),
                "ffprobe не прочитал видео {video:?}"
            );
        }
        let _ = std::fs::remove_file(&video);
    }
}
