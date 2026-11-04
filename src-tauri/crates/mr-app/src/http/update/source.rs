use std::time::Duration;

use serde_json::{json, Value};

pub(super) const REPO: &str = "guest363/podtekst";
const LATEST_JSON: &str =
    "https://github.com/guest363/podtekst/releases/latest/download/latest.json";

#[derive(Debug)]
pub(super) struct UpdateError {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) retryable: bool,
    pub(super) retry_after_sec: Option<u64>,
    pub(super) hint: Option<&'static str>,
}

impl UpdateError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: true,
            retry_after_sec: None,
            hint: None,
        }
    }

    fn with_hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    pub(super) fn json(&self) -> Value {
        json!({
            "error": self.message,
            "error_code": self.code,
            "stage": "check",
            "retryable": self.retryable,
            "retry_after_sec": self.retry_after_sec,
            "hint": self.hint,
        })
    }
}

fn parse_version(text: &str) -> Vec<u64> {
    text.trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|part| part.parse::<u64>().ok())
        .collect()
}

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

fn platform_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("windows", "x86_64") => Some("windows-x86_64"),
        ("macos", "aarch64") => Some("darwin-aarch64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        _ => None,
    }
}

fn classify_http(code: u16, status_text: &str) -> UpdateError {
    match code {
        403 => UpdateError::new(
            "github_forbidden",
            "GitHub временно ограничил проверку обновлений.",
        )
        .with_hint("Повторите позже или скачайте пакет со страницы релиза."),
        404 => {
            let mut err = UpdateError::new("not_found", "Релиз не найден.");
            err.retryable = false;
            err.hint = Some("Откройте страницу релизов и скачайте пакет вручную.");
            err
        }
        429 => {
            let mut err = UpdateError::new(
                "rate_limited",
                "GitHub временно ограничил проверку обновлений.",
            );
            err.retry_after_sec = Some(60);
            err.hint = Some("Повторите позже или скачайте пакет со страницы релиза.");
            err
        }
        500..=599 => UpdateError::new(
            "github_unavailable",
            format!("GitHub временно недоступен ({code} {status_text})."),
        ),
        _ => UpdateError::new(
            "bad_response",
            format!("Неожиданный ответ GitHub ({code} {status_text})."),
        ),
    }
}

fn map_ureq_error(err: ureq::Error) -> UpdateError {
    match err {
        ureq::Error::Status(code, response) => {
            let mut mapped = classify_http(code, response.status_text());
            if code == 403
                && response
                    .header("x-ratelimit-remaining")
                    .is_some_and(|v| v.trim() == "0")
            {
                mapped.code = "rate_limited";
                mapped.retry_after_sec = response
                    .header("x-ratelimit-reset")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .and_then(|reset| reset.checked_sub(chrono::Utc::now().timestamp() as u64));
            }
            mapped
        }
        ureq::Error::Transport(t) => {
            UpdateError::new("offline", format!("Нет связи с интернетом: {t}"))
                .with_hint("Проверьте подключение и повторите.")
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .build()
}

fn read_json(url: &str, accept: &str) -> Result<Value, UpdateError> {
    let text = agent()
        .get(url)
        .set(
            "User-Agent",
            concat!("Podtekst/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", accept)
        .call()
        .map_err(map_ureq_error)?
        .into_string()
        .map_err(|e| {
            UpdateError::new("bad_response", format!("Не удалось прочитать ответ: {e}"))
        })?;
    serde_json::from_str(&text)
        .map_err(|e| UpdateError::new("bad_response", format!("Некорректный JSON: {e}")))
}

fn notes_payload(notes: &str) -> (String, bool) {
    let truncated = notes.chars().count() > 16_000;
    (notes.chars().take(16_000).collect(), truncated)
}

fn not_supported() -> UpdateError {
    let mut err = UpdateError::new(
        "platform_not_supported",
        "Для этой платформы обновление не опубликовано.",
    );
    err.retryable = false;
    err.hint = Some("Скачайте пакет со страницы релиза вручную.");
    err
}

fn fetch_latest_json() -> Result<Value, UpdateError> {
    let release = read_json(LATEST_JSON, "application/json")?;
    let version = release
        .get("version")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| UpdateError::new("bad_response", "В latest.json нет version"))?;
    let key = platform_key().ok_or_else(not_supported)?;
    let target = release
        .get("platforms")
        .and_then(|p| p.get(key))
        .ok_or_else(not_supported)?;
    if target
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .is_empty()
        || target
            .get("signature")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .is_empty()
    {
        return Err(UpdateError::new(
            "updater_unavailable",
            "В latest.json нет ссылки или подписи пакета.",
        ));
    }
    let (notes, notes_truncated) =
        notes_payload(release.get("notes").and_then(|x| x.as_str()).unwrap_or(""));
    let version = version.trim_start_matches(['v', 'V']);
    Ok(json!({
        "latest": version,
        "tag": format!("v{version}"),
        "url": format!("https://github.com/{REPO}/releases/tag/v{version}"),
        "name": format!("Подтекст v{version}"),
        "date": release.get("pub_date").cloned().unwrap_or(Value::Null),
        "notes": notes,
        "notes_truncated": notes_truncated,
        "size_bytes": target.get("size").and_then(|x| x.as_u64()),
        "source": "latest_json",
    }))
}

fn fetch_github() -> Result<Value, UpdateError> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let release = read_json(&url, "application/vnd.github+json")?;
    let tag = release
        .get("tag_name")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| UpdateError::new("bad_response", "В релизе GitHub нет tag_name"))?;
    let (notes, notes_truncated) =
        notes_payload(release.get("body").and_then(|x| x.as_str()).unwrap_or(""));
    Ok(json!({
        "latest": tag.trim_start_matches(['v', 'V']),
        "tag": tag,
        "url": release.get("html_url").and_then(|x| x.as_str())
            .unwrap_or("https://github.com/guest363/podtekst/releases/latest"),
        "name": release.get("name").and_then(|x| x.as_str()).unwrap_or(tag),
        "date": release.get("published_at").cloned().unwrap_or(Value::Null),
        "notes": notes,
        "notes_truncated": notes_truncated,
        "size_bytes": Value::Null,
        "source": "github",
    }))
}

pub(super) fn fetch_latest() -> Result<Value, UpdateError> {
    match fetch_latest_json() {
        Ok(info) => Ok(info),
        Err(primary) => match fetch_github() {
            Ok(info) => Ok(info),
            Err(fallback) => Err(if primary.code == "offline" {
                primary
            } else {
                fallback
            }),
        },
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

    #[test]
    fn platform_key_covers_supported_builds() {
        let key = platform_key();
        if cfg!(target_os = "linux") {
            assert_eq!(key, Some("linux-x86_64"));
        }
        if cfg!(target_os = "windows") {
            assert_eq!(key, Some("windows-x86_64"));
        }
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(key, Some("darwin-aarch64"));
        }
    }

    #[test]
    fn error_classification_is_stable() {
        assert_eq!(classify_http(404, "Not Found").code, "not_found");
        assert_eq!(classify_http(403, "Forbidden").code, "github_forbidden");
        assert_eq!(classify_http(429, "Too Many Requests").code, "rate_limited");
        assert_eq!(
            classify_http(503, "Service Unavailable").code,
            "github_unavailable"
        );
    }
}
