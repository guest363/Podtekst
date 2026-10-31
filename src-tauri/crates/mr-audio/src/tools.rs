//! Поиск внешних инструментов ffmpeg/ffprobe.
//!
//! Порядок: папка `bin` внутри данных приложения (куда кладёт скачивание
//! из интерфейса), затем системный PATH. Ядро нигде не хардкодит имя
//! бинарника, поэтому установка ffmpeg не требует пересборки.

use std::path::PathBuf;
use std::sync::Mutex;

// Mutex, а не OnceLock: на мобильных ОС папка данных известна только после
// инициализации Tauri, и регистрация должна уметь перезаписаться.
static EXTRA_BIN_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Регистрирует дополнительную папку поиска (вызывается ядром при старте).
pub fn set_extra_bin_dir(dir: PathBuf) {
    if let Ok(mut g) = EXTRA_BIN_DIR.lock() {
        *g = Some(dir);
    }
}

fn resolve(tool: &str) -> PathBuf {
    let exe = if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    };
    if let Ok(g) = EXTRA_BIN_DIR.lock() {
        if let Some(dir) = g.as_ref() {
            let p = dir.join(&exe);
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from(tool)
}

/// Путь для запуска ffmpeg: `<данные>/bin`, затем PATH.
pub fn ffmpeg_path() -> PathBuf {
    resolve("ffmpeg")
}

/// Путь для запуска ffprobe: `<данные>/bin`, затем PATH.
pub fn ffprobe_path() -> PathBuf {
    resolve("ffprobe")
}

/// Доступен ли ffmpeg — с кэшем на 30 с (функция зовётся из частых опросов /api/status).
pub fn ffmpeg_available() -> bool {
    static CACHE: Mutex<(bool, Option<std::time::Instant>)> = Mutex::new((false, None));
    const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
    let mut cache = match CACHE.lock() {
        Ok(g) => g,
        Err(_) => return ffmpeg_available_now(),
    };
    if let Some(t) = cache.1 {
        if t.elapsed() < RETRY_AFTER {
            return cache.0;
        }
    }
    cache.1 = Some(std::time::Instant::now());
    cache.0 = ffmpeg_available_now();
    cache.0
}

/// Проверка запуском `-version` без кэша (сразу после установки или удаления).
pub fn ffmpeg_available_now() -> bool {
    std::process::Command::new(ffmpeg_path())
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_extra_bin_dir_then_path() {
        // OnceLock процесса глобален, поэтому оба случая — в одном тесте:
        // сначала фолбэк на PATH, затем приоритет зарегистрированной папки.
        assert_eq!(ffmpeg_path(), PathBuf::from("ffmpeg"));

        let dir = std::env::temp_dir().join(format!("mr-tools-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        std::fs::write(dir.join(exe), b"stub").unwrap();
        set_extra_bin_dir(dir.clone());
        assert_eq!(ffmpeg_path(), dir.join(exe));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
