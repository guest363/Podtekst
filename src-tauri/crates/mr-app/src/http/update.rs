//! Проверка обновлений приложения по последнему GitHub Release.
//!
//! Сравниваем тег релиза с версией, вшитой при сборке (`CARGO_PKG_VERSION`).
//! Ответ кэшируется в памяти на несколько часов: старт приложения не должен
//! дёргать GitHub при каждом перезапуске. `force=1` обходит кэш для кнопки
//! «Проверить обновления».

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::json_response;

const REPO: &str = "guest363/meeting-record";
const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);

type Cache = Mutex<Option<(Instant, Value)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Версия как набор чисел: `v0.2.1-beta` → `[0, 2, 1]`.
fn parse_version(text: &str) -> Vec<u64> {
    text.trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|part| part.parse::<u64>().ok())
        .collect()
}

/// Строгое сравнение версий без внешних крейтов (SemVer здесь не нужен:
/// релизы нумеруются простым `MAJOR.MINOR.PATCH`).
pub(crate) fn version_is_newer(latest: &str, current: &str) -> bool {
    let a = parse_version(latest);
    let b = parse_version(current);
    if a.is_empty() {
        return false;
    }
    for i in 0..a.len().max(b.len()) {
        match a
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&b.get(i).copied().unwrap_or(0))
        {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => {}
        }
    }
    false
}

fn fetch_latest() -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .build();
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let text = agent
        .get(&url)
        .set(
            "User-Agent",
            concat!("MeetingRecorder/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub недоступен: {e}"))?
        .into_string()
        .map_err(|e| format!("Не удалось прочитать ответ GitHub: {e}"))?;
    let release: Value =
        serde_json::from_str(&text).map_err(|e| format!("Некорректный ответ GitHub: {e}"))?;
    let tag = release
        .get("tag_name")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| "В релизе GitHub нет tag_name".to_string())?;
    Ok(json!({
        "latest": tag.trim_start_matches(['v', 'V']),
        "tag": tag,
        "url": release.get("html_url").and_then(|x| x.as_str()).unwrap_or("https://github.com/guest363/meeting-record/releases/latest"),
        "name": release.get("name").and_then(|x| x.as_str()).unwrap_or(tag),
    }))
}

/// GET /api/update/check[?force=1] — есть ли на GitHub версия новее текущей.
pub(super) fn handle_update_check(request: tiny_http::Request, force: bool) {
    let current = env!("CARGO_PKG_VERSION");
    let latest = {
        let cached = if force {
            None
        } else {
            cache()
                .lock()
                .ok()
                .and_then(|g| g.as_ref().cloned())
                .filter(|(at, _)| at.elapsed() < CACHE_TTL)
                .map(|(_, v)| v)
        };
        match cached {
            Some(v) => Ok(v),
            None => fetch_latest().inspect(|v| {
                if let Ok(mut g) = cache().lock() {
                    *g = Some((Instant::now(), v.clone()));
                }
            }),
        }
    };
    match latest {
        Ok(info) => {
            let latest = info.get("latest").and_then(|x| x.as_str()).unwrap_or("");
            json_response(
                request,
                json!({
                    "ok": true,
                    "current": current,
                    "latest": latest,
                    "is_newer": version_is_newer(latest, current),
                    "self_update": self_update::available(),
                    "url": info.get("url"),
                    "name": info.get("name"),
                }),
                200,
            );
        }
        Err(e) => json_response(
            request,
            json!({"ok": false, "current": current, "error": e}),
            200,
        ),
    }
}

/// POST /api/update/apply — скачать и установить обновление. Запускается только
/// по явному нажатию в UI; при успехе приложение перезапускается само.
pub(super) fn handle_update_apply(request: tiny_http::Request) {
    match self_update::apply() {
        Ok(()) => json_response(request, json!({"ok": true}), 200),
        Err(e) => json_response(request, json!({"ok": false, "error": e}), 200),
    }
}

/// GET /api/update/progress — ход скачивания (UI опрашивает раз в секунду).
pub(super) fn handle_update_progress(request: tiny_http::Request) {
    json_response(request, self_update::progress(), 200)
}

/// Передаёт handle окна штатному апдейтеру (вызывается из Tauri `setup`).
#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
pub(crate) fn set_app(app: tauri::AppHandle) {
    self_update::set_app(app);
}

/// Штатный апдейтер Tauri: доступен только в настольной сборке, а на Linux —
/// только для AppImage (там он подменяет файл пакета).
#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
mod self_update {
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
        pct: u32,
        error: Option<String>,
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
        json!({"running": p.running, "pct": p.pct, "error": p.error})
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
            ..Progress::default()
        };
        drop(p);
        tauri::async_runtime::spawn(async move {
            if let Err(e) = install(&app).await {
                if let Ok(mut p) = state.lock() {
                    p.running = false;
                    p.error = Some(e);
                }
                return;
            }
            if let Ok(mut p) = state.lock() {
                p.pct = 100;
            }
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
        update
            .download_and_install(
                |chunk, total| {
                    if let Ok(mut p) = progress_state().lock() {
                        p.downloaded += chunk as u64;
                        p.pct = match total {
                            Some(t) if t > 0 => (p.downloaded * 100 / t).min(100) as u32,
                            _ => 0,
                        };
                    }
                },
                || {},
            )
            .await
            .map_err(|e| format!("Не удалось установить обновление: {e}"))
    }
}

/// Ядро без `tauri-ui` и мобильные сборки: установка недоступна, остаётся ссылка.
#[cfg(not(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
)))]
mod self_update {
    use serde_json::{json, Value};

    pub(super) fn available() -> bool {
        false
    }

    pub(super) fn apply() -> Result<(), String> {
        Err("Автообновление доступно в установленной сборке — скачайте новую версию со страницы релиза".to_string())
    }

    pub(super) fn progress() -> Value {
        json!({"running": false, "pct": 0, "error": null})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_is_numeric_not_lexical() {
        assert!(version_is_newer("0.10.0", "0.9.9"));
        assert!(version_is_newer("v0.2.0", "0.1.0"));
        assert!(!version_is_newer("0.1.0", "0.1.0"));
        assert!(!version_is_newer("0.1.0", "v0.1.0"));
        assert!(!version_is_newer("garbage", "0.1.0"));
    }
}
