//! Проверка обновлений и штатный апдейтер Tauri.
//!
//! Канонический источник — `latest.json` последнего релиза: его же читает
//! Tauri updater, поэтому версия и заметки не расходятся с тем, что реально
//! установится. GitHub API остаётся резервом для старых релизов.

mod source;

#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
mod self_update;

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
        json!({
            "running": false,
            "stage": "idle",
            "pct": 0,
            "downloaded": 0,
            "total": 0,
            "version": null,
            "error": null,
            "error_code": null,
        })
    }
}

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::json_response;
use source::fetch_latest;
pub(crate) use source::version_is_newer;

const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);

type Cache = Mutex<Option<(Instant, Value)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Ручная проверка обновлений не чаще раза в 5 секунд: защита от долбёжки
/// GitHub повторными нажатиями.
fn force_allowed() -> bool {
    static LAST_FORCE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let now = Instant::now();
    let mut last = LAST_FORCE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|at| now.duration_since(at) < Duration::from_secs(5)) {
        return false;
    }
    *last = Some(now);
    true
}

fn save_last_check(info: &Value) {
    let latest = info
        .get("latest")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    crate::config::update_config(|cfg| {
        cfg["last_update_check"] = json!({
            "at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "current": env!("CARGO_PKG_VERSION"),
            "latest": latest,
            "is_newer": version_is_newer(&latest, env!("CARGO_PKG_VERSION")),
        });
    });
}

fn skipped_version() -> Option<String> {
    crate::config::load_config()
        .get("skipped_update_version")
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

pub(crate) fn handle_update_check(request: tiny_http::Request, force: bool) {
    let current = env!("CARGO_PKG_VERSION");
    let requested_force = force;
    let force = force && force_allowed();
    let cached_entry = cache().lock().ok().and_then(|g| g.as_ref().cloned());
    let latest = {
        let cached = match cached_entry {
            Some(_) if force => None,
            // Троттлинг force: отдаём прошлый результат, даже если кэш просрочен.
            Some((_, v)) if requested_force => Some(v),
            Some((at, v)) if at.elapsed() < CACHE_TTL => Some(v),
            _ => None,
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
            save_last_check(&info);
            let latest = info.get("latest").and_then(|x| x.as_str()).unwrap_or("");
            let skipped = skipped_version();
            json_response(
                request,
                json!({
                    "ok": true,
                    "current": current,
                    "latest": latest,
                    "is_newer": version_is_newer(latest, current),
                    "self_update": self_update::available(),
                    "install_mode": if self_update::available() { "self" } else { "manual" },
                    "url": info.get("url"),
                    "name": info.get("name"),
                    "tag": info.get("tag"),
                    "date": info.get("date"),
                    "notes": info.get("notes"),
                    "notes_truncated": info.get("notes_truncated"),
                    "size_bytes": info.get("size_bytes"),
                    "sources_agree": true,
                    "source": info.get("source"),
                    "stale": false,
                    "skipped": skipped.as_deref() == Some(latest),
                    "skipped_version": skipped,
                    "error_code": Value::Null,
                    "retryable": false,
                    "hint": Value::Null,
                }),
                200,
            );
        }
        Err(e) => {
            let cached = crate::config::load_config()
                .get("last_update_check")
                .cloned()
                .unwrap_or(Value::Null);
            let stale = !cached.is_null();
            let mut body = e.json();
            body["ok"] = json!(false);
            body["current"] = json!(current);
            body["self_update"] = json!(self_update::available());
            body["install_mode"] = json!(if self_update::available() {
                "self"
            } else {
                "manual"
            });
            body["skipped_version"] = json!(skipped_version());
            body["cached"] = cached;
            body["stale"] = json!(stale);
            json_response(request, body, 200);
        }
    }
}

pub(crate) fn handle_update_state(request: tiny_http::Request) {
    let current = env!("CARGO_PKG_VERSION");
    let cfg = crate::config::load_config();
    let mut last_check = cfg.get("last_update_check").cloned().unwrap_or(Value::Null);
    if let Some(obj) = last_check.as_object_mut() {
        obj.insert("stale".to_string(), json!(false));
        if obj.get("current").and_then(|x| x.as_str()) != Some(current) {
            obj.insert("stale".to_string(), json!(true));
        }
    }
    let mut attempt = cfg.get("update_attempt").cloned().unwrap_or(Value::Null);
    if let Some(obj) = attempt.as_object_mut() {
        let from = obj.get("from").and_then(|x| x.as_str()).unwrap_or("");
        let to = obj.get("to").and_then(|x| x.as_str()).unwrap_or("");
        let outcome = obj.get("outcome").and_then(|x| x.as_str()).unwrap_or("");
        if outcome == "started" {
            if current == to {
                obj.insert("outcome".to_string(), json!("installed"));
            } else if current == from {
                obj.insert("outcome".to_string(), json!("not_confirmed"));
            }
        }
    }
    json_response(
        request,
        json!({
            "ok": true,
            "current": current,
            "self_update": self_update::available(),
            "install_mode": if self_update::available() { "self" } else { "manual" },
            "last_check": last_check,
            "skipped_version": skipped_version(),
            "last_attempt": attempt,
        }),
        200,
    );
}

pub(crate) fn handle_update_skip(request: tiny_http::Request, body: &Value) {
    let version = body.get("version").and_then(|x| x.as_str()).map(str::trim);
    crate::config::update_config(|cfg| match version {
        Some(v) if !v.is_empty() => cfg["skipped_update_version"] = json!(v),
        _ => {
            if let Some(obj) = cfg.as_object_mut() {
                obj.remove("skipped_update_version");
            }
        }
    });
    json_response(
        request,
        json!({"ok": true, "skipped_version": version.filter(|v| !v.is_empty())}),
        200,
    );
}

fn save_attempt(from: &str, to: &str, outcome: &str) {
    crate::config::update_config(|cfg| {
        cfg["update_attempt"] = json!({
            "from": from,
            "to": to,
            "started_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "outcome": outcome,
        });
    });
}

/// POST /api/update/apply — скачать и установить обновление. Запускается только
/// по явному нажатию в UI; при успехе приложение перезапускается само.
pub(crate) fn handle_update_apply(request: tiny_http::Request, busy: bool) {
    if !self_update::available() {
        json_response(
            request,
            json!({
                "ok": false,
                "error": "Эта сборка не обновляется сама — скачайте пакет со страницы релиза.",
                "error_code": "not_supported_build",
                "stage": "apply",
                "retryable": false,
                "hint": "Обновление доступно в установленной сборке (AppImage, .app, NSIS).",
            }),
            200,
        );
        return;
    }
    if busy {
        json_response(
            request,
            json!({
                "ok": false,
                "error": "Сначала остановите запись или дождитесь окончания расшифровки.",
                "error_code": "busy_recording",
                "stage": "apply",
                "retryable": true,
                "hint": "Остановите запись и повторите.",
            }),
            409,
        );
        return;
    }
    let to = crate::config::load_config()
        .get("last_update_check")
        .and_then(|x| x.get("latest"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    save_attempt(env!("CARGO_PKG_VERSION"), &to, "started");
    match self_update::apply() {
        Ok(()) => json_response(
            request,
            json!({"ok": true, "version": to, "stage": "starting"}),
            200,
        ),
        Err(e) => {
            save_attempt(env!("CARGO_PKG_VERSION"), &to, "failed");
            json_response(
                request,
                json!({
                    "ok": false,
                    "error": e,
                    "error_code": "install_failed",
                    "stage": "apply",
                    "retryable": true,
                    "hint": "Текущая версия продолжает работать. Повторите или скачайте пакет вручную.",
                }),
                200,
            )
        }
    }
}

pub(crate) fn handle_update_progress(request: tiny_http::Request) {
    json_response(request, self_update::progress(), 200);
}

/// Для `/api/status` и смоука: доступно ли самообновление в этой сборке.
pub(crate) fn available_for_status() -> bool {
    self_update::available()
}

#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
pub(crate) fn set_app(app: tauri::AppHandle) {
    self_update::set_app(app);
}
