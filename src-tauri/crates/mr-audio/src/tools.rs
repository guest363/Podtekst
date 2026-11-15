//! Порядок поиска ffmpeg/ffprobe: `<данные>/bin` (сюда кладёт скачивание из
//! интерфейса), затем PATH — установка ffmpeg не требует пересборки ядра.

mod arch;

pub use arch::binary_arch;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PROBE_TTL: Duration = Duration::from_secs(30);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

// Mutex, а не OnceLock: на мобильных ОС папка данных известна только после
// инициализации Tauri, и регистрация должна уметь перезаписаться.
static EXTRA_BIN_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Результат опроса `-version`: UI показывает версию, источник и архитектуру
/// установленного файла, не запуская процесс повторно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolProbe {
    pub version: Option<String>,
    pub raw: String,
    pub source: &'static str,
    pub arch: Option<&'static str>,
    pub path: PathBuf,
}

pub fn set_extra_bin_dir(dir: PathBuf) {
    if let Ok(mut g) = EXTRA_BIN_DIR.lock() {
        *g = Some(dir);
    }
}

fn bin_name(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

fn resolve(tool: &str) -> PathBuf {
    if let Ok(g) = EXTRA_BIN_DIR.lock() {
        if let Some(dir) = g.as_ref() {
            let p = dir.join(bin_name(tool));
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from(tool)
}

pub fn ffmpeg_path() -> PathBuf {
    resolve("ffmpeg")
}

pub fn ffprobe_path() -> PathBuf {
    resolve("ffprobe")
}

/// Версия и паспорт ffmpeg — с кэшем на 30 с: `/api/status` опрашивается часто,
/// а один запуск `-version` стоит десятки миллисекунд.
pub fn ffmpeg_version() -> Option<ToolProbe> {
    probe_cached("ffmpeg")
}

pub fn ffprobe_version() -> Option<ToolProbe> {
    probe_cached("ffprobe")
}

/// Контракт: сброс кэша после установки, замены или удаления бинарника.
pub fn invalidate() {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Cache::default();
    }
}

/// Доступен ли ffmpeg: опрос версии уже запускает процесс, отдельная проверка
/// запустила бы второй.
pub fn ffmpeg_available() -> bool {
    ffmpeg_version().is_some()
}

/// Контракт: опрос в обход кэша после установки или удаления бинарника.
pub fn ffmpeg_available_now() -> bool {
    probe_tool("ffmpeg").is_some()
}

/// `-version` конкретного файла с пределом 5 с: битый бинарник зависает.
pub fn probe_at(path: &Path) -> Option<ToolProbe> {
    let raw = run_version(path)?;
    Some(ToolProbe {
        version: parse_version(&raw),
        raw,
        source: source_of(path),
        arch: binary_arch(path),
        path: path.to_path_buf(),
    })
}

/// `major.minor` для сверки пары ffmpeg/ffprobe: патч-версии у сборок расходятся.
pub fn major_minor(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

#[derive(Default)]
struct Cache {
    ffmpeg: Option<(Instant, Option<ToolProbe>)>,
    ffprobe: Option<(Instant, Option<ToolProbe>)>,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    ffmpeg: None,
    ffprobe: None,
});

fn probe_cached(tool: &str) -> Option<ToolProbe> {
    let mut cache = match CACHE.lock() {
        Ok(g) => g,
        Err(_) => return probe_tool(tool),
    };
    let slot = if tool == "ffmpeg" {
        &mut cache.ffmpeg
    } else {
        &mut cache.ffprobe
    };
    if let Some((at, probe)) = slot.as_ref() {
        if at.elapsed() < PROBE_TTL {
            return probe.clone();
        }
    }
    let probe = probe_tool(tool);
    *slot = Some((Instant::now(), probe.clone()));
    probe
}

fn probe_tool(tool: &str) -> Option<ToolProbe> {
    probe_at(&located(tool))
}

/// Путь к исполняемому файлу: папка приложения, затем PATH. Для PATH ищем
/// абсолютный путь, иначе заголовок архитектуры не прочитать.
fn located(tool: &str) -> PathBuf {
    let app = resolve(tool);
    if app != Path::new(tool) {
        return app;
    }
    let exe = bin_name(tool);
    if let Some(paths) = std::env::var_os("PATH") {
        if let Some(found) = std::env::split_paths(&paths)
            .map(|dir| dir.join(&exe))
            .find(|p| p.is_file())
        {
            return found;
        }
    }
    app
}

fn source_of(path: &Path) -> &'static str {
    if let Ok(g) = EXTRA_BIN_DIR.lock() {
        if let Some(dir) = g.as_ref() {
            if path.parent() == Some(dir.as_path()) {
                return "app";
            }
        }
    }
    "path"
}

fn run_version(path: &Path) -> Option<String> {
    let mut child = std::process::Command::new(path)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = wait_bounded(&mut child, PROBE_TIMEOUT)?;
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let line = out.lines().find(|l| !l.trim().is_empty())?.trim();
    Some(line.to_string())
}

/// Ограничение ОС: `status()` без предела зависает на битом бинарнике.
fn wait_bounded(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn parse_version(raw: &str) -> Option<String> {
    let line = raw.lines().next()?;
    let mut words = line.split_whitespace();
    if !words.next()?.starts_with("ff") {
        return None;
    }
    if words.next()? != "version" {
        return None;
    }
    version_token(words.next()?)
}

pub fn version_token(token: &str) -> Option<String> {
    // JVS пишет `n7.0.2-static`, поэтому префикс `n` — часть версии.
    let token = token.strip_prefix('n').unwrap_or(token);
    let core: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let core = core.trim_matches('.');
    if core.is_empty() {
        // Ночные сборки: `N-113341-g...`, `git-...`.
        return None;
    }
    Some(core.to_string())
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
        let exe = bin_name("ffmpeg");
        std::fs::write(dir.join(&exe), b"stub").unwrap();
        set_extra_bin_dir(dir.clone());
        assert_eq!(ffmpeg_path(), dir.join(&exe));
        assert_eq!(source_of(&ffmpeg_path()), "app");
        assert_eq!(source_of(Path::new("/usr/bin/ffmpeg")), "path");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_line_variants() {
        assert_eq!(
            parse_version("ffmpeg version 9.0.2 Copyright (c) 2000-2025").as_deref(),
            Some("9.0.2")
        );
        assert_eq!(
            parse_version("ffmpeg version 7.0.2-static https://johnvansickle.com").as_deref(),
            Some("7.0.2")
        );
        assert_eq!(
            parse_version("ffmpeg version n7.0.2-static https://johnvansickle.com").as_deref(),
            Some("7.0.2")
        );
        assert_eq!(
            parse_version("ffmpeg version 9.0.2-tessus").as_deref(),
            Some("9.0.2")
        );
        assert_eq!(
            parse_version("ffprobe version 9.0.2 Copyright (c) 2007-2025"),
            Some("9.0.2".to_string())
        );
        assert_eq!(
            parse_version("ffmpeg version N-113341-g8f2c1d3ef2 Copyright"),
            None
        );
        assert_eq!(parse_version("ffmpeg version git-2024-01-01"), None);
        assert_eq!(parse_version("not a probe"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn versions_compare_by_major_minor() {
        assert_eq!(major_minor("9.0.2"), Some((9, 0)));
        assert_eq!(major_minor("7.0"), Some((7, 0)));
        assert_eq!(major_minor("N-113341"), None);
        assert_eq!(major_minor(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_times_out() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 5"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("sh");
        let start = Instant::now();
        assert!(wait_bounded(&mut child, Duration::from_millis(100)).is_none());
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "ожидание не ограничено: {:?}",
            start.elapsed()
        );
    }
}
