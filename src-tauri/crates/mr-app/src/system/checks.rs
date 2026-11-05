use std::time::Instant;

use serde_json::{json, Value};

const FIX_KINDS: [&str; 6] = [
    "open_folder",
    "open_settings",
    "download_model",
    "download_ffmpeg",
    "open_update",
    "collect_diagnostics",
];

pub(super) struct Outcome {
    pub(super) status: &'static str,
    pub(super) detail: String,
    pub(super) hint: String,
    pub(super) fix: Value,
}

impl Outcome {
    pub(super) fn ok(detail: impl Into<String>) -> Self {
        Self {
            status: "ok",
            detail: detail.into(),
            hint: String::new(),
            fix: Value::Null,
        }
    }

    pub(super) fn warn(detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            status: "warn",
            detail: detail.into(),
            hint: hint.into(),
            fix: Value::Null,
        }
    }

    pub(super) fn error(detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            status: "error",
            detail: detail.into(),
            hint: hint.into(),
            fix: Value::Null,
        }
    }

    pub(super) fn skip(detail: impl Into<String>) -> Self {
        Self {
            status: "skip",
            detail: detail.into(),
            hint: String::new(),
            fix: Value::Null,
        }
    }

    pub(super) fn with_fix(mut self, kind: &str, target: &str, label: &str) -> Self {
        self.fix = fix(kind, target, label);
        self
    }
}

fn fix(kind: &str, target: &str, label: &str) -> Value {
    debug_assert!(
        FIX_KINDS.contains(&kind),
        "недопустимый вид исправления: {kind}"
    );
    json!({"kind": kind, "target": target, "label": label})
}

macro_rules! run_check {
    ($checks:expr, $id:expr, $title:expr, $out:expr) => {{
        let started = Instant::now();
        let outcome = $out;
        $checks.push(json!({
            "id": $id,
            "title": $title,
            "status": outcome.status,
            "detail": outcome.detail,
            "hint": outcome.hint,
            "fix": outcome.fix,
            "duration_ms": started.elapsed().as_millis() as u64,
        }));
    }};
}

pub(super) fn run(
    snapshot: &Value,
    probe_audio: bool,
    recording: bool,
    include_optional: bool,
) -> Value {
    let mut checks = Vec::with_capacity(10);
    run_check!(
        checks,
        "storage",
        "Свободное место",
        super::health::check_storage(snapshot)
    );
    run_check!(
        checks,
        "models",
        "Модели ИИ",
        super::health::check_models(snapshot)
    );
    run_check!(
        checks,
        "ffmpeg",
        "ffmpeg",
        super::health::check_ffmpeg(snapshot)
    );
    run_check!(
        checks,
        "audio_input",
        "Микрофон",
        check_audio_input(snapshot)
    );
    run_check!(
        checks,
        "system_audio",
        "Системный звук",
        check_system_audio(snapshot)
    );
    run_check!(
        checks,
        "microphone_probe",
        "Проверка микрофона",
        check_microphone(probe_audio, recording)
    );
    run_check!(
        checks,
        "screen_capture",
        "Запись экрана",
        check_screen_capture(snapshot, include_optional)
    );
    run_check!(
        checks,
        "integration",
        "Интеграция с системой",
        super::health::check_integration(snapshot)
    );
    run_check!(
        checks,
        "updates",
        "Обновления",
        super::health::check_updates(snapshot)
    );
    run_check!(
        checks,
        "journal_crash",
        "Журнал и настройки",
        super::health::check_journal(snapshot)
    );

    let count = |status: &str| {
        checks
            .iter()
            .filter(|check| check["status"] == status)
            .count()
    };
    let (ok, warn, error, skip) = (count("ok"), count("warn"), count("error"), count("skip"));
    let status = if error > 0 {
        "error"
    } else if warn > 0 {
        "warn"
    } else if ok > 0 {
        "ok"
    } else {
        "skip"
    };
    json!({
        "ok": true,
        "checked_at": now_iso(),
        "checks": checks,
        "summary": {
            "status": status,
            "ok": ok,
            "warn": warn,
            "error": error,
            "skip": skip,
        },
    })
}

fn check_audio_input(snapshot: &Value) -> Outcome {
    let inputs = snapshot
        .pointer("/devices/inputs")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let default = snapshot
        .pointer("/devices/default_input")
        .and_then(Value::as_str);
    match (inputs, default) {
        (0, _) => Outcome::error(
            "Аудиовходы не найдены",
            "Подключите микрофон и проверьте настройки звука системы",
        )
        .with_fix(
            "open_settings",
            mic_settings_target(),
            "Настройки микрофона",
        ),
        (_, None) => Outcome::warn(
            format!("Входов: {inputs}, но вход по умолчанию не выбран"),
            "Выберите устройство ввода в настройках системы",
        )
        .with_fix(
            "open_settings",
            mic_settings_target(),
            "Настройки микрофона",
        ),
        _ => Outcome::ok(format!("Аудиовходов: {inputs}, вход по умолчанию выбран")),
    }
}

fn check_system_audio(snapshot: &Value) -> Outcome {
    let permission = snapshot
        .pointer("/permissions/system_audio")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let has_device = snapshot
        .pointer("/devices/has_system_audio")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !permission {
        return Outcome::error(
            "Нет разрешения на запись системного звука",
            "Разрешите запись экрана и системного аудио в настройках конфиденциальности",
        )
        .with_fix(
            "open_settings",
            "macos-screen-recording",
            "Открыть настройки",
        );
    }
    if !has_device {
        return Outcome::warn(
            "Системный звук не найден",
            "Собеседник будет слышен только через микрофон",
        )
        .with_fix("open_settings", sound_settings_target(), "Настройки звука");
    }
    Outcome::ok("Системный звук доступен")
}

fn check_microphone(probe_audio: bool, recording: bool) -> Outcome {
    if !probe_audio {
        return Outcome::skip("Проверка захвата не запрашивалась");
    }
    if recording {
        return Outcome::skip("Идёт запись — проверка пропущена");
    }
    // Android: AAudio-проба из HTTP-потока роняет процесс на части прошивок.
    // Живой микрофон там проверяется обычной записью — она идёт через сервис.
    #[cfg(target_os = "android")]
    {
        Outcome::skip("На телефоне микрофон проверяется короткой записью, а не тестом")
    }
    #[cfg(not(target_os = "android"))]
    {
        super::probe::run()
    }
}

fn check_screen_capture(snapshot: &Value, include_optional: bool) -> Outcome {
    if !include_optional {
        return Outcome::skip("Опциональная проверка не запрашивалась");
    }
    let supported = snapshot
        .pointer("/permissions/screen_capture_supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if supported {
        return Outcome::ok("Запись экрана доступна");
    }
    let mut outcome = Outcome::warn(
        "Запись экрана недоступна",
        if cfg!(target_os = "macos") {
            "Разрешите «Запись экрана» в настройках конфиденциальности"
        } else if cfg!(windows) {
            "Нужен ffmpeg с кодировщиком H.264"
        } else {
            "Запись экрана работает в GNOME на Wayland"
        },
    );
    if cfg!(target_os = "macos") {
        outcome = outcome.with_fix(
            "open_settings",
            "macos-screen-recording",
            "Открыть настройки",
        );
    }
    outcome
}

fn mic_settings_target() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-microphone"
    } else if cfg!(windows) {
        "windows-microphone-privacy"
    } else {
        "linux-sound"
    }
}

fn sound_settings_target() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-screen-recording"
    } else if cfg!(windows) {
        "windows-sound"
    } else {
        "linux-sound"
    }
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Value {
        json!({
            "disk": {"free_bytes": 10u64 * 1024 * 1024 * 1024},
            "models": {
                "core_ready": true,
                "installed": 4,
                "total": 6,
                "missing_core": [],
                "updates": [],
            },
            "ffmpeg": {"available": true, "downloading": false},
            "devices": {
                "inputs": [{"id": "mic", "name": "Микрофон", "muted": false, "system": false}],
                "system": [],
                "default_input": "mic",
                "default_system": null,
                "has_system_audio": false,
            },
            "permissions": {"system_audio": true, "screen_capture_supported": true},
            "integration": {
                "autostart": true,
                "hotkey": {"accelerator": "Ctrl+Shift+R", "registered": null},
                "single_instance": true,
            },
            "update": {"current": "0.4.1", "mode": "manual", "latest": null},
            "crashes": {"count": 0},
            "journal": {"count": 0},
        })
    }

    #[test]
    fn synthetic_snapshot_runs_all_checks() {
        let value = run(&snapshot(), false, false, false);
        let checks = value["checks"].as_array().expect("checks");
        assert_eq!(checks.len(), 10);
        for check in checks {
            assert!(
                matches!(
                    check["status"].as_str(),
                    Some("ok" | "warn" | "error" | "skip")
                ),
                "неизвестный статус: {check}"
            );
            assert!(check.get("fix").is_some(), "нет поля fix: {check}");
        }
        let ids: Vec<&str> = checks.iter().filter_map(|c| c["id"].as_str()).collect();
        assert_eq!(ids.first(), Some(&"storage"));
        assert_eq!(ids.last(), Some(&"journal_crash"));
    }

    #[test]
    fn microphone_probe_is_skipped_when_not_requested() {
        assert_eq!(check_microphone(false, false).status, "skip");
        assert_eq!(check_microphone(false, true).status, "skip");
        assert_eq!(check_microphone(true, true).status, "skip");
    }
}
