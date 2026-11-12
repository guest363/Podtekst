use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::runtime_locator::{self, RuntimeInfo};
use super::ApiError;

mod archive;

#[derive(Debug, Clone, Deserialize)]
struct Pack {
    id: String,
    backend: String,
    title: String,
    description: String,
    asset: String,
    url: String,
    sha256: String,
    compressed_bytes: u64,
    installed_bytes: u64,
    format: String,
    library: String,
    min_free_mb: u64,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    build: String,
    commit: String,
    license: String,
    license_url: String,
    packs: std::collections::BTreeMap<String, Pack>,
}

fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!("../../summary-gpu-manifest.json"))
            .expect("summary-gpu-manifest.json вшит и валиден")
    })
}

fn pack_for_target() -> Option<&'static Pack> {
    manifest().packs.get(runtime_locator::target_key())
}

pub fn supported() -> bool {
    cfg!(target_os = "macos")
        || (!cfg!(target_os = "android") && !cfg!(target_os = "ios") && pack_for_target().is_some())
}

fn bundled() -> bool {
    cfg!(target_os = "macos")
}

fn pack_dir() -> PathBuf {
    crate::config::app_data_dir()
        .join("runtime")
        .join("gpu")
        .join(runtime_locator::triple())
}

fn state_path() -> PathBuf {
    pack_dir().join("state.json")
}

fn library_path(pack: &Pack) -> PathBuf {
    pack_dir().join(&pack.library)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PackState {
    build: String,
    commit: String,
    version: String,
    backend: String,
    #[serde(default)]
    device: Option<String>,
    installed_at: String,
}

fn read_state() -> Option<PackState> {
    let text = std::fs::read_to_string(state_path()).ok()?;
    serde_json::from_str(&text).ok()
}

struct InstallState {
    installing: bool,
    pct: u8,
    stage: String,
    error: Option<String>,
    error_code: Option<String>,
    cancel: Arc<AtomicBool>,
}

static INSTALL: Mutex<Option<InstallState>> = Mutex::new(None);

fn stage(pct: u8, text: &str) {
    let mut guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.pct = state.pct.max(pct.min(99));
        state.stage = text.to_string();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Device {
    index: u32,
    name: String,
    free_mb: u64,
}

fn parse_devices(text: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.contains("Metal") {
            devices.push(Device {
                index: 0,
                name: "Metal".to_string(),
                free_mb: u64::MAX,
            });
            continue;
        }
        let Some(rest) = line.strip_prefix("Vulkan") else {
            continue;
        };
        let Some((index, rest)) = rest.split_once(':') else {
            continue;
        };
        let Ok(index) = index.trim().parse::<u32>() else {
            continue;
        };
        let free_mb = rest
            .rsplit_once(" MiB free")
            .and_then(|(before, _)| {
                before
                    .rsplit(|c: char| !c.is_ascii_digit())
                    .next()
                    .and_then(|n| n.parse().ok())
            })
            .unwrap_or(0);
        let name = rest
            .rsplit_once('(')
            .map(|(name, _)| name)
            .unwrap_or(rest)
            .trim()
            .to_string();
        devices.push(Device {
            index,
            name,
            free_mb,
        });
    }
    devices
}

/// На гибридном ноутбуке `Vulkan0` часто встроенная графика. Предпочитаем
/// дискретную карту, иначе — максимум свободной памяти.
fn choose_device(devices: &[Device]) -> Option<&Device> {
    let discrete = |name: &str| {
        let name = name.to_ascii_lowercase();
        if matches!(name.as_str(), "amd radeon(tm) graphics" | "amd radeon graphics") {
            return false;
        }
        ["nvidia", "geforce", "rtx", "radeon", "amd", "arc"]
            .iter()
            .any(|needle| name.contains(needle))
    };
    devices
        .iter()
        .filter(|d| discrete(&d.name))
        .max_by_key(|d| d.free_mb)
        .or_else(|| devices.iter().max_by_key(|d| d.free_mb))
}

/// Установленный и проверенный Vulkan-backend. Возвращает CPU-runtime,
/// к которому подключается плагин: второй llama-server не нужен.
pub struct GpuRuntime {
    pub runtime: RuntimeInfo,
    pub backend: Option<PathBuf>,
    pub device_index: Option<u32>,
}

pub fn installed_runtime() -> Option<GpuRuntime> {
    if !supported() {
        return None;
    }
    let info = runtime_locator::locate().ok()?;
    if bundled() {
        let devices = runtime_locator::list_devices(&info.exe, &info.dir).ok()?;
        return choose_device(&parse_devices(&devices)).map(|_| GpuRuntime {
            runtime: info.clone(),
            backend: None,
            device_index: None,
        });
    }
    let pack = pack_for_target()?;
    let state = read_state()?;
    if state.build != manifest().build || state.commit != manifest().commit {
        return None;
    }
    let library = library_path(pack);
    if !library.is_file() {
        return None;
    }
    let text =
        runtime_locator::list_devices_with_backend(&info.exe, &info.dir, Some(&library)).ok()?;
    choose_device(&parse_devices(&text)).map(|device| GpuRuntime {
        runtime: info.clone(),
        backend: Some(library),
        device_index: Some(device.index),
    })
}

pub fn status_json() -> Value {
    let pack = pack_for_target();
    let state = read_state();
    let install = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let bundled = bundled();
    let installed = if bundled {
        runtime_locator::locate().is_ok()
    } else {
        pack.as_ref()
            .is_some_and(|pack| library_path(pack).is_file())
            && state.is_some()
    };
    let has_update = !bundled
        && state
            .as_ref()
            .is_some_and(|s| s.build != manifest().build || s.commit != manifest().commit);
    let device = if bundled {
        Some("Apple Silicon (Metal)".to_string())
    } else {
        state.as_ref().and_then(|s| s.device.clone())
    };
    let reason = if !supported() {
        Some("unsupported")
    } else if bundled {
        None
    } else if !installed || has_update {
        Some("not_installed")
    } else if device.is_none() {
        Some("no_device")
    } else {
        None
    };
    let installed_bytes = pack.as_ref().map(|p| {
        library_path(p)
            .metadata()
            .map(|m| m.len())
            .unwrap_or(p.installed_bytes)
    });
    let (installing, pct, stage, error, error_code) = install
        .as_ref()
        .map(|s| {
            (
                s.installing,
                s.pct,
                s.stage.clone(),
                s.error.clone(),
                s.error_code.clone(),
            )
        })
        .unwrap_or((false, 0, String::new(), None, None));
    json!({
        "supported": supported(),
        "bundled": bundled,
        "backend": if bundled { "metal" } else { pack.map(|p| p.backend.as_str()).unwrap_or("vulkan") },
        "title": if bundled { "Metal (встроено)".to_string() } else { pack.map(|p| p.title.clone()).unwrap_or_else(|| "GPU".to_string()) },
        "description": pack.map(|p| p.description.clone()).unwrap_or_else(|| "Metal встроен в macOS.".to_string()),
        "pack_id": pack.map(|p| p.id.clone()),
        "installed": installed,
        "has_update": has_update,
        "device": device,
        "reason": reason,
        "compressed_bytes": pack.map(|p| p.compressed_bytes).unwrap_or(0),
        "installed_bytes": installed_bytes.unwrap_or(0),
        "min_free_mb": pack.map(|p| p.min_free_mb).unwrap_or(0),
        "license": manifest().license,
        "license_url": manifest().license_url,
        "installing": installing,
        "pct": pct,
        "stage": stage,
        "error": error,
        "error_code": error_code,
        "cancelled": error_code.as_deref() == Some("gpu_cancelled"),
    })
}

pub fn is_installing() -> bool {
    INSTALL
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|s| s.installing)
}

pub fn install() -> Result<(), ApiError> {
    if super::is_active() {
        return Err(ApiError::new(
            409,
            "gpu_summary_busy",
            "Идёт сборка итогов. Дождитесь окончания или остановите её.",
        ));
    }
    if !supported() {
        return Err(ApiError::new(
            501,
            "gpu_unsupported",
            "Ускорение GPU на этой платформе не поддерживается.",
        ));
    }
    if bundled() {
        return Err(ApiError::new(
            400,
            "gpu_bundled",
            "Metal уже встроен в приложение.",
        ));
    }
    let pack = pack_for_target().cloned().ok_or_else(|| {
        ApiError::new(501, "gpu_unsupported", "Для этой платформы нет GPU-пакета.")
    })?;
    let mut guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().is_some_and(|s| s.installing) {
        return Err(ApiError::new(
            409,
            "gpu_busy",
            "GPU-пакет уже скачивается. Дождитесь окончания.",
        ));
    }
    let dir = pack_dir();
    std::fs::create_dir_all(&dir).map_err(|_| {
        ApiError::new(
            500,
            "gpu_internal",
            "Не удалось создать папку для GPU-пакета.",
        )
    })?;
    let need = pack.min_free_mb.saturating_mul(1024 * 1024);
    if fs2::available_space(&dir)
        .ok()
        .is_some_and(|free| free < need)
    {
        return Err(ApiError::new(
            422,
            "gpu_disk_low",
            "Не хватает места на диске для GPU-пакета.",
        ));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *guard = Some(InstallState {
        installing: true,
        pct: 0,
        stage: "Скачивание…".to_string(),
        error: None,
        error_code: None,
        cancel: cancel.clone(),
    });
    drop(guard);
    let worker_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("summary-gpu".to_string())
        .spawn(move || {
            let result = install_worker(&pack, &worker_cancel);
            let mut guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
            let Some(state) = guard.as_mut() else {
                return;
            };
            if !Arc::ptr_eq(&state.cancel, &worker_cancel) {
                return;
            }
            state.installing = false;
            match result {
                Ok(()) => {
                    state.pct = 100;
                    state.stage = "Готово".to_string();
                    state.error = None;
                    state.error_code = None;
                }
                Err(error) => {
                    crate::note!(
                        "summary: GPU install failed: code={} detail={}",
                        error.code(),
                        error.detail()
                    );
                    state.stage = String::new();
                    state.error = Some(error.message());
                    state.error_code = Some(error.code().to_string());
                }
            }
        })
        .map_err(|_| {
            *INSTALL.lock().unwrap_or_else(|e| e.into_inner()) = None;
            ApiError::new(
                500,
                "gpu_internal",
                "Не удалось запустить установку GPU-пакета.",
            )
        })?;
    Ok(())
}

pub fn cancel() -> bool {
    let guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = guard.as_ref() else {
        return false;
    };
    if !state.installing {
        return false;
    }
    state.cancel.store(true, Ordering::Relaxed);
    true
}

pub fn remove() -> Result<Value, ApiError> {
    if !supported() {
        return Err(ApiError::new(
            501,
            "gpu_unsupported",
            "Ускорение GPU на этой платформе не поддерживается.",
        ));
    }
    if bundled() {
        return Err(ApiError::new(
            400,
            "gpu_bundled",
            "Metal встроен в приложение и не удаляется.",
        ));
    }
    if super::is_active() {
        return Err(ApiError::new(
            409,
            "gpu_summary_busy",
            "Идёт сборка итогов. Дождитесь окончания или остановите её.",
        ));
    }
    {
        let guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_some_and(|s| s.installing) {
            return Err(ApiError::new(
                409,
                "gpu_busy",
                "Сначала дождитесь окончания установки GPU-пакета.",
            ));
        }
    }
    super::sidecar::kill_current();
    let dir = pack_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|_| {
            ApiError::new(
                500,
                "gpu_remove_failed",
                "Не удалось удалить GPU-пакет. Закройте приложение и повторите.",
            )
        })?;
    }
    *INSTALL.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(status_json())
}

#[derive(Debug)]
enum InstallError {
    Cancelled,
    Busy,
    Net(String),
    Sha(String),
    Extract(String),
    Probe(String),
    Io(String),
}

impl InstallError {
    fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "gpu_cancelled",
            Self::Busy => "gpu_summary_busy",
            Self::Net(_) => "gpu_download_failed",
            Self::Sha(_) => "gpu_sha_mismatch",
            Self::Extract(_) => "gpu_extract_failed",
            Self::Probe(_) => "gpu_probe_failed",
            Self::Io(_) => "gpu_internal",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Cancelled | Self::Busy => "",
            Self::Net(detail)
            | Self::Sha(detail)
            | Self::Extract(detail)
            | Self::Probe(detail)
            | Self::Io(detail) => detail,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Cancelled => "Скачивание отменено.".to_string(),
            Self::Busy => {
                "Идёт сборка итогов. Дождитесь окончания или остановите её.".to_string()
            }
            Self::Net(_) => "Не удалось скачать GPU-пакет. Проверьте сеть и повторите.".to_string(),
            Self::Sha(_) => "Файл GPU-пакета повреждён. Повторите скачивание.".to_string(),
            Self::Extract(_) => "Не удалось распаковать GPU-пакет. Повторите скачивание.".to_string(),
            Self::Probe(_) => {
                "GPU-пакет установлен, но Vulkan-устройство не найдено. Итоги продолжат работать на CPU."
                    .to_string()
            }
            Self::Io(_) => "Не удалось установить GPU-пакет. Повторите.".to_string(),
        }
    }
}

fn install_worker(pack: &Pack, cancel: &AtomicBool) -> Result<(), InstallError> {
    let dir = pack_dir();
    std::fs::create_dir_all(&dir).map_err(|e| InstallError::Io(e.to_string()))?;
    stage(0, "Скачивание…");
    let archive = mr_models::download_asset(
        &dir,
        &pack.asset,
        &pack.url,
        pack.compressed_bytes,
        &pack.sha256,
        &|| cancel.load(Ordering::Relaxed),
        &|ev| stage((ev.pct as u32 * 80 / 100) as u8, "Скачивание…"),
    )
    .map_err(|e| match e {
        mr_models::ModelsError::Cancelled => InstallError::Cancelled,
        mr_models::ModelsError::ShaMismatch { .. } => InstallError::Sha(e.to_string()),
        mr_models::ModelsError::Net(_) => InstallError::Net(e.to_string()),
        other => InstallError::Io(other.to_string()),
    })?;
    if cancel.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    stage(82, "Проверяю SHA-256…");
    if super::is_active() {
        return Err(InstallError::Busy);
    }
    // Windows не даст заменить загруженную DLL: гасим sidecar до распаковки.
    super::sidecar::kill_current();
    stage(86, "Распаковываю…");
    let library = archive::extract_library(&archive, &pack.format, &pack.library, &dir)
        .map_err(InstallError::Extract)?;
    let len = std::fs::metadata(&library)
        .map(|m| m.len())
        .map_err(|e| InstallError::Io(e.to_string()))?;
    if pack.installed_bytes > 0 && len != pack.installed_bytes {
        return Err(InstallError::Extract(format!(
            "размер {}: {len} вместо {}",
            pack.library, pack.installed_bytes
        )));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    stage(93, "Проверяю Vulkan…");
    let info = runtime_locator::locate()
        .map_err(|e| InstallError::Probe(format!("CPU runtime: {e:?}")))?;
    let devices = runtime_locator::list_devices_with_backend(&info.exe, &info.dir, Some(&library))
        .map_err(|e| InstallError::Probe(format!("--list-devices: {e:?}")))?;
    let device = choose_device(&parse_devices(&devices)).map(|d| d.name.clone());
    stage(97, "Устанавливаю…");
    let state = PackState {
        build: manifest().build.clone(),
        commit: manifest().commit.clone(),
        version: manifest().version.clone(),
        backend: pack.backend.clone(),
        device,
        installed_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    let bytes = serde_json::to_vec_pretty(&state).map_err(|e| InstallError::Io(e.to_string()))?;
    crate::fsutil::write_atomic(&state_path(), &bytes)
        .map_err(|e| InstallError::Io(e.to_string()))?;
    let _ = std::fs::remove_file(&archive);
    stage(100, "Готово");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_matches_runtime_pin() {
        let runtime: serde_json::Value =
            serde_json::from_str(include_str!("../../summary-runtime-manifest.json")).unwrap();
        assert_eq!(manifest().build, runtime["build"]);
        assert_eq!(manifest().commit, runtime["commit"]);
        assert_eq!(manifest().version, runtime["version"]);
        assert!(!manifest().packs.is_empty());
    }

    #[test]
    fn pack_is_well_formed() {
        for (key, pack) in &manifest().packs {
            assert!(pack
                .url
                .starts_with("https://github.com/ggml-org/llama.cpp/releases/download/"));
            assert_eq!(pack.sha256.len(), 64);
            assert!(pack.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(pack.compressed_bytes > 0);
            assert!(pack.installed_bytes > 0);
            assert!(matches!(pack.format.as_str(), "tar.gz" | "zip"));
            assert!(!pack.library.is_empty(), "{key}");
            assert!(pack.url.ends_with(&pack.asset));
        }
    }

    #[test]
    fn device_lines_are_parsed_and_discrete_is_preferred() {
        let text = "Available devices:\n  Vulkan0: Intel(R) Iris(R) Xe Graphics (23859 MiB, 13809 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 4060 (8188 MiB, 7000 MiB free)\n";
        let devices = parse_devices(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].index, 0);
        assert_eq!(devices[1].free_mb, 7000);
        assert_eq!(choose_device(&devices).map(|d| d.index), Some(1));
        assert_eq!(
            choose_device(&parse_devices("Available devices:\n  (none)\n")),
            None
        );
    }

    #[test]
    fn generic_radeon_graphics_does_not_outrank_discrete_gpu() {
        let devices = parse_devices(
            "Available devices:\n  Vulkan0: NVIDIA GeForce RTX 4060 (7956 MiB, 7188 MiB free)\n  Vulkan1: AMD Radeon(TM) Graphics (16200 MiB, 15390 MiB free)\n",
        );
        assert_eq!(choose_device(&devices).map(|d| d.index), Some(0));
        assert_eq!(choose_device(&devices[1..]).map(|d| d.index), Some(1));
    }

    #[test]
    fn unsafe_archive_entries_are_rejected() {
        assert!(!archive::safe_entry("../evil"));
        assert!(!archive::safe_entry("/etc/passwd"));
        assert!(!archive::safe_entry("-rf"));
        assert!(archive::safe_entry("llama-b12182/libggml-vulkan.so"));
    }

    #[cfg(unix)]
    #[test]
    fn tar_extracts_only_the_backend_library() {
        let base = std::env::temp_dir().join(format!(
            "mr-gpu-tar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let src = base.join("src/pkg");
        let dest = base.join("dest");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("libggml-vulkan.so"), b"vulkan-backend").unwrap();
        let archive = base.join("pack.tar.gz");
        let status = std::process::Command::new("tar")
            .args(["-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(base.join("src"))
            .arg("pkg")
            .status()
            .unwrap();
        assert!(status.success());
        let path = archive::extract_library(&archive, "tar.gz", "libggml-vulkan.so", &dest)
            .expect("tar extraction");
        assert_eq!(std::fs::read(path).unwrap(), b"vulkan-backend");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn zip_extracts_only_the_backend_library() {
        use std::io::Write;
        let base = std::env::temp_dir().join(format!(
            "mr-gpu-zip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("pack.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("pkg/ggml-vulkan.dll", options).unwrap();
        zip.write_all(b"vulkan-backend").unwrap();
        zip.finish().unwrap();
        let dest = base.join("dest");
        let path = archive::extract_library(&archive, "zip", "ggml-vulkan.dll", &dest)
            .expect("zip extraction");
        assert_eq!(std::fs::read(path).unwrap(), b"vulkan-backend");
        let _ = std::fs::remove_dir_all(&base);
    }
}
