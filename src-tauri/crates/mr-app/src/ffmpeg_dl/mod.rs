//! Бинарники кладутся в `<данные>/bin`, проверяются запуском `-version`
//! и подхватываются ядром через `mr_audio::tools::ffmpeg_path()` без пересборки.

mod install;
mod pins;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mr_audio::tools::{binary_arch, probe_at, ToolProbe};
use serde::{Deserialize, Serialize};

/// Страница ручной установки — подсказка в интерфейсе при неудаче автоскачивания.
pub(crate) const MANUAL_URL: &str = "https://www.gyan.dev/ffmpeg/builds/";

#[derive(Default)]
struct Dl {
    done: u64,
    total: u64,
    error: Option<String>,
}

static DL: Mutex<Option<Dl>> = Mutex::new(None);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Installed {
    version: String,
    arch: String,
    source: String,
    installed_at: String,
    #[serde(default)]
    files: BTreeMap<String, String>,
}

fn bin_dir() -> PathBuf {
    crate::config::app_data_dir().join("bin")
}

fn bin_name(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

pub(crate) fn status_json() -> serde_json::Value {
    let ffmpeg = mr_audio::tools::ffmpeg_version();
    let ffprobe = mr_audio::tools::ffprobe_version();
    let packages = pins::current().ok();
    let expected = packages.as_deref().and_then(pins::expected_version);
    let state = read_state(&bin_dir());
    let (downloading, pct, error) = {
        let guard = DL.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(d) if d.error.is_none() => (
                true,
                (d.done * 95)
                    .checked_div(d.total)
                    .map(|v| v as u8)
                    .unwrap_or(0),
                serde_json::Value::Null,
            ),
            Some(d) => (false, 0, serde_json::json!(d.error)),
            None => (false, 0, serde_json::Value::Null),
        }
    };
    serde_json::json!({
        "available": ffmpeg.is_some(),
        "downloading": downloading,
        "pct": pct.min(95),
        "error": error,
        "manual_url": MANUAL_URL,
        "version": ffmpeg.as_ref().and_then(|p| p.version.clone()),
        "version_raw": ffmpeg.as_ref().map(|p| p.raw.clone()),
        "ffprobe_version": ffprobe.as_ref().and_then(|p| p.version.clone()),
        "source": ffmpeg.as_ref().map(|p| p.source),
        "arch": ffmpeg.as_ref().and_then(|p| p.arch),
        "expected_version": expected,
        "can_update": needs_update(state.as_ref(), ffmpeg.as_ref(), ffprobe.as_ref(), expected.as_deref()),
        "pair_ok": pair_ok(ffmpeg.as_ref(), ffprobe.as_ref()),
    })
}

/// Установлен из `<bin>`, но пины сборки другие: UI предлагает «Обновить».
fn needs_update(
    state: Option<&Installed>,
    ffmpeg: Option<&ToolProbe>,
    ffprobe: Option<&ToolProbe>,
    expected: Option<&str>,
) -> bool {
    let (Some(expected), Some(ffmpeg)) = (expected, ffmpeg) else {
        return false;
    };
    if ffmpeg.source != "app" {
        return false;
    }
    let arch = std::env::consts::ARCH;
    match state {
        Some(state) => state.version != expected || state.arch != arch || ffprobe.is_none(),
        None => {
            ffprobe.is_none()
                || ffmpeg.version.as_deref() != Some(expected)
                || ffmpeg.arch != Some(arch)
        }
    }
}

fn pair_ok(ffmpeg: Option<&ToolProbe>, ffprobe: Option<&ToolProbe>) -> bool {
    let (Some(ffmpeg), Some(ffprobe)) = (ffmpeg, ffprobe) else {
        return false;
    };
    let (Some(ffmpeg), Some(ffprobe)) = (ffmpeg.version.as_deref(), ffprobe.version.as_deref())
    else {
        return false;
    };
    match (
        mr_audio::tools::major_minor(ffmpeg),
        mr_audio::tools::major_minor(ffprobe),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

pub(crate) fn start_download(force: bool, recording: bool, screen: bool) -> Result<(), String> {
    if let Some(message) = blocker(recording, screen) {
        return Err(message);
    }
    {
        let mut guard = DL.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|d| d.error.is_none()).unwrap_or(false) {
            return Err("Скачивание уже идёт".to_string());
        }
        *guard = Some(Dl::default());
    }
    std::thread::Builder::new()
        .name("ffmpeg-dl".into())
        .spawn(move || download_worker(force))
        .map_err(|e| format!("Не удалось запустить поток скачивания: {e}"))?;
    Ok(())
}

/// Замена бинарников ломает идущую работу: процесс держит файл, а расшифровка
/// и запись экрана вызывают ffmpeg прямо сейчас.
fn blocker(recording: bool, screen: bool) -> Option<String> {
    if recording {
        return Some("Идёт запись. Остановите запись и повторите.".to_string());
    }
    if screen {
        return Some("Идёт запись экрана. Остановите запись и повторите.".to_string());
    }
    #[cfg(feature = "onnx")]
    if crate::transcribe::has_pending_work() {
        return Some(
            "Идёт расшифровка или в очереди есть файлы. Дождитесь окончания и повторите."
                .to_string(),
        );
    }
    if crate::summary::is_active() {
        return Some("Идут итоги. Дождитесь окончания и повторите.".to_string());
    }
    None
}

fn set_progress(done: u64, total: u64) {
    if let Ok(mut g) = DL.lock() {
        if let Some(d) = g.as_mut() {
            d.done = done;
            d.total = total;
        }
    }
}

fn download_worker(force: bool) {
    let err = download_and_install(force);
    let mut guard = DL.lock().unwrap_or_else(|e| e.into_inner());
    if err.is_none() {
        *guard = None;
    } else if let Some(d) = guard.as_mut() {
        d.error = err;
    }
}

fn download_and_install(force: bool) -> Option<String> {
    let packages = match pins::current() {
        Ok(packages) => packages,
        Err(error) => return Some(error),
    };
    let bin = bin_dir();
    let mut state = read_state(&bin);
    let mut installed = false;
    for package in &packages {
        if !force && package_current(package, &bin, state.as_ref()) {
            continue;
        }
        crate::note!("[FFMPEG] Ставлю {} из {}", package.version, package.url);
        let files = match install_package(package, &bin) {
            Ok(files) => files,
            Err(error) => return Some(format!("Источник {}: {error}", package.source)),
        };
        installed = true;
        let mut next = state.take().unwrap_or_default();
        next.version = package.version.clone();
        next.arch = std::env::consts::ARCH.to_string();
        next.source = package.source.clone();
        next.installed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        next.files.extend(files);
        if let Err(error) = write_state(&bin, &next) {
            return Some(error);
        }
        state = Some(next);
    }
    if installed {
        mr_audio::tools::invalidate();
        crate::screen::invalidate_encoders();
        crate::http::media::invalidate_durations();
        crate::note!("[FFMPEG] Готово: {}", bin.display());
    }
    None
}

fn package_current(package: &pins::Package, bin: &Path, state: Option<&Installed>) -> bool {
    let arch = std::env::consts::ARCH;
    state.is_some_and(|s| s.version == package.version && s.arch == arch)
        && package
            .provides
            .iter()
            .all(|name| bin.join(bin_name(name)).is_file())
}

fn install_package(
    package: &pins::Package,
    bin: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let work = std::env::temp_dir().join(format!("mr-ffmpeg-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&work).map_err(|e| format!("временная папка: {e}"))?;
    let result = install_package_in(package, bin, &work);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn install_package_in(
    package: &pins::Package,
    bin: &Path,
    work: &Path,
) -> Result<BTreeMap<String, String>, String> {
    std::fs::create_dir_all(bin).map_err(|e| format!("bin: {e}"))?;
    let archive = install::download(package, work, &mut set_progress)?;
    install_archive(package, &archive, bin, work)
}

fn install_archive(
    package: &pins::Package,
    archive: &Path,
    bin: &Path,
    work: &Path,
) -> Result<BTreeMap<String, String>, String> {
    install::verify_archive(package, archive)?;
    let unpacked = work.join("unpacked");
    install::extract(package, archive, &unpacked)?;

    let mut staged = Staged::default();
    let mut hashes = BTreeMap::new();
    for name in &package.provides {
        let source =
            install::find_binary(&unpacked, name).ok_or_else(|| format!("в архиве нет {name}"))?;
        let dest = bin.join(bin_name(name));
        let file_name = dest
            .file_name()
            .ok_or_else(|| format!("bin: недопустимый путь {}", dest.display()))?;
        let new = dest.with_file_name(format!("{}.new", file_name.to_string_lossy()));
        let _ = std::fs::remove_file(&new);
        std::fs::copy(&source, &new).map_err(|e| format!("копия {name}: {e}"))?;
        staged.0.push((new.clone(), dest));
        make_executable(&new);
        if binary_arch(&new) != Some(std::env::consts::ARCH) {
            return Err(format!(
                "{name}: архитектура не совпадает с {}",
                std::env::consts::ARCH
            ));
        }
        if probe_at(&new).is_none() {
            return Err(format!("{name}: бинарник не запускается"));
        }
        hashes.insert(
            name.clone(),
            mr_models::sha256_of_file(&new).map_err(|e| format!("{name}: {e}"))?,
        );
    }
    for (new, dest) in &staged.0 {
        std::fs::rename(new, dest).map_err(|e| format!("замена {}: {e}", dest.display()))?;
    }
    Ok(hashes)
}

/// Неудачная проверка оставляет старый рабочий файл: `.new` удаляется на выходе,
/// старый бинарник не трогаем до успешного `rename`.
#[derive(Default)]
struct Staged(Vec<(PathBuf, PathBuf)>);

impl Drop for Staged {
    fn drop(&mut self) {
        for (new, _) in &self.0 {
            let _ = std::fs::remove_file(new);
        }
    }
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

fn state_path(bin: &Path) -> PathBuf {
    bin.join("state.json")
}

fn read_state(bin: &Path) -> Option<Installed> {
    let text = std::fs::read_to_string(state_path(bin)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_state(bin: &Path, state: &Installed) -> Result<(), String> {
    std::fs::create_dir_all(bin).map_err(|e| format!("bin: {e}"))?;
    let path = state_path(bin);
    let tmp = path.with_file_name(format!("state.json.tmp.{}", std::process::id()));
    let body = serde_json::to_vec_pretty(state).map_err(|e| format!("state.json: {e}"))?;
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("state.json: {e}"))?;
        file.write_all(&body)
            .map_err(|e| format!("state.json: {e}"))?;
        file.sync_all().map_err(|e| format!("state.json: {e}"))?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("state.json: {e}"))
}

#[cfg(test)]
mod tests;
