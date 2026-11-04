use serde_json::Value;

use crate::config::app_data_dir;

use super::checks::Outcome;

const MB: f64 = 1024.0 * 1024.0;
const GB: f64 = 1024.0 * MB;

pub(super) fn check_storage(snapshot: &Value) -> Outcome {
    let Some(free) = snapshot.pointer("/disk/free_bytes").and_then(Value::as_u64) else {
        return Outcome::warn(
            "Не удалось определить свободное место",
            "Проверьте права на папку записей",
        )
        .with_fix("open_folder", "recordings", "Открыть папку записей");
    };
    let free_gb = free as f64 / GB;
    if free < 500 * 1024 * 1024 {
        Outcome::error(
            format!("Свободно {free_gb:.1} ГБ — запись может прерваться"),
            "Освободите место на диске или смените папку записей",
        )
        .with_fix("open_folder", "recordings", "Открыть папку записей")
    } else if free < 2 * 1024 * 1024 * 1024 {
        Outcome::warn(
            format!("Свободно {free_gb:.1} ГБ — маловато для длинной встречи"),
            "Освободите несколько гигабайт перед длинной записью",
        )
        .with_fix("open_folder", "recordings", "Открыть папку записей")
    } else {
        Outcome::ok(format!("Свободно {free_gb:.1} ГБ"))
    }
}

pub(super) fn check_models(snapshot: &Value) -> Outcome {
    let core_ready = snapshot
        .pointer("/models/core_ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let missing = strings(snapshot, "/models/missing_core");
    let updates = strings(snapshot, "/models/updates");
    if !core_ready {
        let detail = if missing.is_empty() {
            "Базовые модели не найдены".to_string()
        } else {
            format!("Не хватает базовых моделей: {}", missing.join(", "))
        };
        return Outcome::error(detail, "Скачайте базовые модели в разделе «Модели ИИ»").with_fix(
            "download_model",
            missing.first().map(String::as_str).unwrap_or(""),
            "Скачать модели",
        );
    }
    if let Some(first) = updates.first() {
        return Outcome::warn(
            format!("Доступны обновления моделей: {}", updates.join(", ")),
            "Обновите модели, чтобы получить исправления",
        )
        .with_fix("download_model", first, "Обновить модели");
    }
    let installed = snapshot
        .pointer("/models/installed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = snapshot
        .pointer("/models/total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Outcome::ok(format!("Базовые модели готовы ({installed} из {total})"))
}

pub(super) fn check_ffmpeg(snapshot: &Value) -> Outcome {
    let available = snapshot
        .pointer("/ffmpeg/available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let downloading = snapshot
        .pointer("/ffmpeg/downloading")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if available {
        return Outcome::ok("ffmpeg найден");
    }
    if downloading {
        return Outcome::warn("ffmpeg скачивается", "Дождитесь окончания загрузки");
    }
    Outcome::warn(
        "ffmpeg не найден",
        "Без ffmpeg не откроются видео и часть форматов аудио",
    )
    .with_fix("download_ffmpeg", "ffmpeg", "Скачать ffmpeg")
}

pub(super) fn check_integration(snapshot: &Value) -> Outcome {
    let autostart = snapshot
        .pointer("/integration/autostart")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let accelerator = snapshot
        .pointer("/integration/hotkey/accelerator")
        .and_then(Value::as_str)
        .unwrap_or("");
    let single_instance = snapshot
        .pointer("/integration/single_instance")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let detail = format!(
        "Хоткей {accelerator} (регистрация не проверена), автозапуск {}, один экземпляр {}",
        if autostart {
            "включён"
        } else {
            "выключен — это нормально"
        },
        if single_instance { "да" } else { "нет" }
    );
    Outcome::ok(detail)
}

pub(super) fn check_updates(snapshot: &Value) -> Outcome {
    let current = snapshot
        .pointer("/update/current")
        .and_then(Value::as_str)
        .unwrap_or("");
    let latest = snapshot
        .pointer("/update/latest")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mode = snapshot
        .pointer("/update/mode")
        .and_then(Value::as_str)
        .unwrap_or("manual");
    let skipped = snapshot
        .pointer("/update/skipped_version")
        .and_then(Value::as_str)
        .unwrap_or("");
    if latest.is_empty() || !crate::http::update::version_is_newer(latest, current) {
        return Outcome::ok(format!("Установлена версия {current}; обновление: {mode}"));
    }
    if skipped == latest {
        return Outcome::ok(format!(
            "Установлена версия {current}; версия {latest} пропущена пользователем"
        ));
    }
    Outcome::warn(
        format!("Доступна версия {latest} (установлена {current})"),
        "Обновитесь, чтобы получить исправления",
    )
    .with_fix("open_update", latest, "Открыть обновление")
}

pub(super) fn check_journal(snapshot: &Value) -> Outcome {
    let crashes = snapshot
        .pointer("/crashes/count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let journal = snapshot
        .pointer("/journal/count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let config = app_data_dir().join("config.json");
    let config_state = match std::fs::read_to_string(&config) {
        Ok(text) if serde_json::from_str::<Value>(&text).is_ok() => {
            Ok("настройки читаются".to_string())
        }
        Ok(_) => Err("config.json повреждён".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok("настройки по умолчанию".to_string())
        }
        Err(e) => Err(format!("config.json не читается: {e}")),
    };
    let config_detail =
        match config_state {
            Ok(detail) => detail,
            Err(e) => {
                return Outcome::error(e, "Соберите диагностику и проверьте файл настроек")
                    .with_fix("collect_diagnostics", "", "Собрать диагностику")
            }
        };
    if crashes == 0 {
        return Outcome::ok(format!(
            "Неразобранных сбоев нет; файлов журнала: {journal}; {config_detail}"
        ));
    }
    Outcome::warn(
        format!("Неразобранных сбоев: {crashes}; файлов журнала: {journal}; {config_detail}"),
        "Проверьте журнал или соберите диагностику",
    )
    .with_fix("collect_diagnostics", "", "Собрать диагностику")
}

fn strings(snapshot: &Value, pointer: &str) -> Vec<String> {
    snapshot
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn storage_shortage_is_error() {
        let value = json!({"disk": {"free_bytes": 100u64 * 1024 * 1024}});
        assert_eq!(check_storage(&value).status, "error");
        let value = json!({"disk": {"free_bytes": 1024u64 * 1024 * 1024}});
        assert_eq!(check_storage(&value).status, "warn");
    }

    #[test]
    fn models_and_updates_use_snapshot() {
        let value = json!({"models": {"core_ready": false, "missing_core": ["vad-silero-v6"]}});
        assert_eq!(check_models(&value).status, "error");
        let value = json!({"models": {"core_ready": true, "updates": ["asr-gigaam-v3-rnnt-int8"]}});
        assert_eq!(check_models(&value).status, "warn");
        let value = json!({
            "update": {"current": "0.4.1", "mode": "manual", "latest": "0.5.0"},
        });
        assert_eq!(check_updates(&value).status, "warn");
        let value = json!({
            "update": {"current": "0.5.0", "mode": "manual", "latest": "0.4.1"},
        });
        assert_eq!(check_updates(&value).status, "ok");
    }

    #[test]
    fn disabled_autostart_is_not_a_problem() {
        let value = json!({
            "integration": {
                "hotkey": {"accelerator": "Ctrl+Shift+R"},
                "autostart": false,
                "single_instance": true,
            },
        });
        assert_eq!(check_integration(&value).status, "ok");
    }
}
