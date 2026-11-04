use std::time::Duration;

use crate::config::app_data_dir;

use super::checks::Outcome;

pub(super) fn run() -> Outcome {
    let catalog = mr_audio::capture::devices();
    let Some(mic) = catalog.default_input.clone() else {
        return Outcome::error(
            "Микрофон по умолчанию не найден",
            "Подключите микрофон и повторите проверку",
        );
    };
    let dir =
        app_data_dir()
            .join("tmp")
            .join(format!("probe-{}-{}", std::process::id(), now_nanos()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Outcome::error(
            format!("Не создана временная папка: {e}"),
            "Проверьте права на папку данных приложения",
        );
    }
    let request = mr_audio::capture::CaptureRequest {
        rate: 16000,
        mic: Some(mic),
        system: catalog.default_system.clone(),
        mic_path: dir.join("mic.f32"),
        system_path: dir.join("sys.f32"),
    };
    let outcome = match mr_audio::capture::start_capture(&request) {
        Ok(mut session) => {
            std::thread::sleep(Duration::from_millis(1500));
            session.stop();
            let system_error = session.system_error.clone();
            drop(session);
            match mr_audio::capture::read_raw_f32(&request.mic_path) {
                Ok(samples) if samples.is_empty() => Outcome::warn(
                    "Захват не вернул данных",
                    "Проверьте устройство ввода и его уровень",
                ),
                Ok(samples) => {
                    let peak = samples.iter().fold(0.0f32, |acc, x| acc.max(x.abs()));
                    if peak < 1e-4 {
                        Outcome::warn(
                            "Микрофон записал тишину",
                            "Проверьте уровень микрофона и разрешение на доступ",
                        )
                    } else if let Some(error) = system_error {
                        Outcome::warn(
                            format!("Микрофон работает (пик {peak:.3}), системный звук: {error}"),
                            "Проверьте настройки системного звука",
                        )
                    } else {
                        Outcome::ok(format!(
                            "Захвачено {} сэмплов, пик {peak:.3}",
                            samples.len()
                        ))
                    }
                }
                Err(e) => Outcome::error(
                    format!("Не удалось прочитать запись: {e}"),
                    "Повторите проверку или перезапустите приложение",
                ),
            }
        }
        Err(e) => Outcome::error(
            format!("Захват не стартовал: {e}"),
            "Проверьте микрофон и настройки доступа к нему",
        ),
    };
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
