#[cfg(not(target_os = "android"))]
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::config::app_data_dir;

#[cfg(not(target_os = "android"))]
use super::checks::Outcome;

#[cfg(not(target_os = "android"))]
struct ProbeDir(PathBuf);

#[cfg(not(target_os = "android"))]
impl Drop for ProbeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn cleanup_stale() {
    let root = app_data_dir().join("tmp");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("probe-") {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > Duration::from_secs(15 * 60));
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(not(target_os = "android"))]
pub(super) fn run() -> Outcome {
    // Открытие потока libpulse — блокирующий вызов без таймаута: уводим пробу
    // в поток, чтобы HTTP-запрос и UI не ждали зависший звуковой сервер.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(capture_probe());
    });
    match rx.recv_timeout(Duration::from_secs(12)) {
        Ok(outcome) => outcome,
        Err(_) => Outcome::warn(
            "Проверка микрофона не завершилась за 12 с",
            "Повторите проверку или перезапустите приложение",
        ),
    }
}

#[cfg(not(target_os = "android"))]
fn capture_probe() -> Outcome {
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
    let _guard = ProbeDir(dir.clone());
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
    outcome
}

#[cfg(not(target_os = "android"))]
fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
