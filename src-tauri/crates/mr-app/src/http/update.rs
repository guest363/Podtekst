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
