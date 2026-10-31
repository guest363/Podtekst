//! Скачивание ffmpeg по кнопке из интерфейса (Настройки → Модели ИИ).
//!
//! Источники — публичные сборки, свои на каждую ОС: Windows — essentials
//! от gyan (меньше размер) с зеркалом BtbN; Linux — статические сборки
//! johnvansickle (amd64/arm64) с зеркалом BtbN; macOS — evermeet.cx
//! (ffmpeg и ffprobe отдельными архивами, сборки Intel — на Apple Silicon
//! работают через Rosetta). Архив распаковывается встроенным `tar`
//! (bsdtar: zip/tar.gz/tar.xz; запасной путь на Windows — PowerShell),
//! бинарники кладутся в `<данные>/bin` и проверяются запуском `-version`.
//! Ядро подхватывает их через `mr_audio::tools::ffmpeg_path()` без пересборки.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Что кладёт архив: оба бинарника или один.
enum Provides {
    Both,
    Single(&'static str),
}

struct Group {
    provides: Provides,
    urls: &'static [&'static str],
}

/// Источники текущей ОС: первая сработавшая ссылка группы ставит свои бинарники.
fn sources() -> Vec<Group> {
    const GYAN_WIN: &str = "https://github.com/GyanD/codexffmpeg/releases/download/7.1.1/ffmpeg-7.1.1-essentials_build.zip";
    const BTBN_WIN: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
    const JVS_AMD64: &str = "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz";
    const JVS_ARM64: &str = "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-arm64-static.tar.xz";
    const BTBN_LINUX: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz";
    const EM_FFMPEG: &str = "https://evermeet.cx/ffmpeg/ffmpeg-9.0.2.zip";
    const EM_FFPROBE: &str = "https://evermeet.cx/ffmpeg/ffprobe-9.0.2.zip";
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => vec![Group {
            provides: Provides::Both,
            urls: &[GYAN_WIN, BTBN_WIN],
        }],
        ("linux", "aarch64") => vec![Group {
            provides: Provides::Both,
            urls: &[JVS_ARM64],
        }],
        ("linux", _) => vec![Group {
            provides: Provides::Both,
            urls: &[JVS_AMD64, BTBN_LINUX],
        }],
        ("macos", _) => vec![
            Group {
                provides: Provides::Single("ffmpeg"),
                urls: &[EM_FFMPEG],
            },
            Group {
                provides: Provides::Single("ffprobe"),
                urls: &[EM_FFPROBE],
            },
        ],
        _ => vec![],
    }
}

/// Имя бинарника в `<данные>/bin` с учётом платформы.
fn bin_name(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

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
    let groups = sources();
    if groups.is_empty() {
        return Some("автоскачивание не настроено для этой ОС".to_string());
    }
    let dest_bin = crate::config::app_data_dir().join("bin");
    let mut last = String::from("источники не сработали");
    for group in &groups {
        // Группа ставит только недостающие бинарники: частично установленная
        // конфигурация доустанавливается без повторной загрузки готового.
        let wanted: Vec<&str> = match group.provides {
            Provides::Both => vec!["ffmpeg", "ffprobe"],
            Provides::Single(name) => vec![name],
        }
        .into_iter()
        .filter(|n| !dest_bin.join(bin_name(n)).is_file())
        .collect();
        if wanted.is_empty() {
            continue;
        }
        let mut installed = false;
        for url in group.urls {
            eprintln!("[FFMPEG] Пробую источник: {url}");
            match try_install(url, &wanted, &dest_bin) {
                Ok(names) => {
                    installed = !names.is_empty();
                    break;
                }
                Err(e) => {
                    eprintln!("[FFMPEG] Источник не сработал: {e}");
                    last = e;
                }
            }
        }
        if installed
            && ["ffmpeg", "ffprobe"]
                .iter()
                .all(|n| dest_bin.join(bin_name(n)).is_file())
        {
            return None;
        }
    }
    Some(format!("Все источники недоступны: {last}"))
}

fn try_install(url: &str, wanted: &[&str], dest_bin: &Path) -> Result<Vec<String>, String> {
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
        let n = reader.read(&mut buf).map_err(|e| format!("чтение: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("запись: {e}"))?;
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
    extract_archive(&tmp_zip, &tmp_dir)?;

    std::fs::create_dir_all(dest_bin).map_err(|e| format!("bin: {e}"))?;
    let mut installed = Vec::new();
    for name in wanted {
        let src = find_binary(&tmp_dir, name).ok_or_else(|| format!("в архиве нет {name}"))?;
        let dest = dest_bin.join(bin_name(name));
        std::fs::copy(&src, &dest).map_err(|e| format!("копия {name}: {e}"))?;
        make_executable(&dest);
        // Проверка делом: скопированный бинарник должен запуститься
        let runs = std::process::Command::new(&dest)
            .arg("-version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !runs {
            let _ = std::fs::remove_file(&dest);
            return Err(format!("{name} скопирован, но не запускается (битая сборка?)"));
        }
        installed.push(name.to_string());
    }

    let _ = std::fs::remove_file(&tmp_zip);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    eprintln!("[FFMPEG] Установлено в {}: {}", dest_bin.display(), installed.join(", "));
    Ok(installed)
}

/// Права на исполнение (zip не всегда сохраняет бит исполнения на unix).
fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Распаковка архива (zip/tar.gz/tar.xz — tar распознаёт сам):
/// сначала встроенный `tar`, на Windows запасной путь — PowerShell.
fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    let ok = std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
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
        archive.display(),
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

/// Рекурсивный поиск бинарника в распакованной папке. Имена зависят от
/// сборщика: `ffmpeg`, `ffmpeg.exe` или версионированное `ffmpeg-9.0.2`
/// (evermeet). Ищем точное имя, суффикс .exe и префикс `имя-`.
fn find_binary(root: &Path, tool: &str) -> Option<PathBuf> {
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
                let exe = format!("{tool}.exe");
                let versioned = format!("{tool}-");
                if name == tool || name == exe || name.starts_with(&versioned) {
                    return Some(p);
                }
            }
        }
    }
    None
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
        extract_archive(&zip, &out).expect("распаковка");
        let ff = find_binary(&out, "ffmpeg").expect("ffmpeg найден во вложенной папке");
        let fp = find_binary(&out, "ffprobe").expect("ffprobe найден во вложенной папке");
        let dest = base.join("bin");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::copy(&ff, dest.join("ffmpeg.exe")).unwrap();
        std::fs::copy(&fp, dest.join("ffprobe.exe")).unwrap();
        assert!(dest.join("ffmpeg.exe").is_file());
        assert!(dest.join("ffprobe.exe").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Unix-макеты без сети: tar.gz с бинарниками без .exe и версионированное
    /// имя (как у evermeet) должны находиться; зеркала по ОС не пустые.
    #[test]
    fn tgz_and_versioned_names_and_sources() {
        let base = std::env::temp_dir().join(format!("mr-ffmpeg-tgz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src/ffmpeg-9.0.2-amd64-static");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("ffmpeg"), b"ELF stub").unwrap();
        std::fs::write(src.join("ffprobe"), b"ELF stub").unwrap();
        let tgz = base.join("ffmpeg.tar.gz");
        let st = std::process::Command::new("tar")
            .args(["-czf"])
            .arg(&tgz)
            .arg("-C")
            .arg(base.join("src"))
            .arg("ffmpeg-9.0.2-amd64-static")
            .status()
            .expect("tar");
        assert!(st.success(), "tar не смог собрать tar.gz");

        let out = base.join("out");
        std::fs::create_dir_all(&out).unwrap();
        extract_archive(&tgz, &out).expect("распаковка tar.gz");
        let ff = find_binary(&out, "ffmpeg").expect("версионированное имя найдено");
        assert!(ff.file_name().unwrap().to_string_lossy().contains("ffmpeg"));

        // Для текущей ОС источники заданы: группы не пустые, у каждой есть зеркала
        let groups = sources();
        assert!(!groups.is_empty(), "источники текущей ОС не настроены");
        assert!(groups.iter().all(|g| !g.urls.is_empty()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
