use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use tauri_plugin_updater::UpdaterExt;

/// Окно (и его AppHandle) создаётся после HTTP-сервера — кладём handle в setup.
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static PROGRESS: OnceLock<Mutex<Progress>> = OnceLock::new();

#[derive(Default, Clone)]
struct Progress {
    running: bool,
    downloaded: u64,
    total: u64,
    pct: u32,
    stage: &'static str,
    version: Option<String>,
    error: Option<String>,
    error_code: Option<&'static str>,
}

pub(super) fn set_app(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

/// Можно ли обновиться самим: у запуска из исходников (`run.sh`, cargo) нет
/// ни пакета, ни установленного бандла — там остаётся ссылка на релиз.
pub(super) fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        use tauri::utils::{config::BundleType, platform::bundle_type};
        std::env::var_os("APPIMAGE").is_some() && bundle_type() == Some(BundleType::AppImage)
    }
    #[cfg(target_os = "windows")]
    {
        use tauri::utils::{config::BundleType, platform::bundle_type};
        matches!(bundle_type(), Some(BundleType::Nsis | BundleType::Msi))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        false
    }
}

pub(super) fn progress() -> Value {
    let p = progress_state()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    json!({
        "running": p.running,
        "stage": p.stage,
        "pct": p.pct,
        "downloaded": p.downloaded,
        "total": p.total,
        "version": p.version,
        "error": p.error,
        "error_code": p.error_code,
    })
}

fn progress_state() -> &'static Mutex<Progress> {
    PROGRESS.get_or_init(|| Mutex::new(Progress::default()))
}

pub(super) fn apply() -> Result<(), String> {
    let app = APP.get().cloned().ok_or("Окно приложения ещё не готово")?;
    let state = progress_state();
    let mut p = state.lock().map_err(|e| e.to_string())?;
    if p.running {
        return Err("Обновление уже скачивается".to_string());
    }
    *p = Progress {
        running: true,
        stage: "checking",
        ..Progress::default()
    };
    drop(p);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = install(&app).await {
            if let Ok(mut p) = state.lock() {
                p.running = false;
                p.stage = "error";
                p.error = Some(e);
                p.error_code = Some("install_failed");
            }
            return;
        }
        if let Ok(mut p) = state.lock() {
            p.pct = 100;
            p.stage = "restart";
        }
        // app.restart завершает процесс без возврата из run_cli: закрываем маркер сами.
        crate::crash::mark_clean();
        // Не возвращается: новый процесс уже с обновлённой версией.
        app.restart();
    });
    Ok(())
}

async fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let updater = app
        .updater()
        .map_err(|e| format!("Апдейтер недоступен: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("Не удалось проверить обновления: {e}"))?
        .ok_or("Установлена последняя версия")?;
    if let Ok(mut p) = progress_state().lock() {
        p.version = Some(update.version.clone());
        p.stage = "download";
    }
    update
        .download_and_install(
            |chunk, total| {
                if let Ok(mut p) = progress_state().lock() {
                    p.downloaded += chunk as u64;
                    p.total = total.unwrap_or(0);
                    p.pct = match total {
                        Some(t) if t > 0 => (p.downloaded * 100 / t).min(100) as u32,
                        _ => 0,
                    };
                }
            },
            || {
                if let Ok(mut p) = progress_state().lock() {
                    p.stage = "install";
                }
            },
        )
        .await
        .map_err(|e| format!("Не удалось установить обновление: {e}"))
}
