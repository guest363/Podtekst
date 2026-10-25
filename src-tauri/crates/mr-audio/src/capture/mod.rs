//! Единый слой захвата звука для всех ОС.
//!
//! Каждая платформа реализует один и тот же контракт:
//! * Linux — PipeWire/PulseAudio через `libpulse` (загружается в рантайме),
//!   без внешних утилит `pw-record`/`pactl`; резервный путь — CLI-утилиты,
//!   если системной библиотеки нет.
//! * Windows — WASAPI (пока заглушка: импорт готовых файлов работает,
//!   живой захват возвращает понятную ошибку).
//! * macOS — CoreAudio (пока заглушка: импорт готовых файлов работает,
//!   живой захват возвращает понятную ошибку).
//!
//! Общий результат: моно f32 16 кГц в промежуточном файле + измеритель
//! уровня. Сборка в стерео WAV делается при остановке записи.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use super::{AudioError, Result};
use stream::LevelMeter;

pub mod stream;

#[cfg(target_os = "linux")]
mod cli;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub mod pulse;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "android")]
mod android;

pub use stream::{read_raw_f32, resample_to};

/// Устройство звука, доступное для захвата.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Идентификатор устройства: его нужно передать обратно в `start_capture`.
    pub id: String,
    /// Человекочитаемое имя (как в настройках системы).
    pub name: String,
    /// Устройство заглушено в системе.
    pub muted: bool,
    /// Это системный звук (монитор выхода / loopback / tap), а не микрофон.
    pub system: bool,
}

/// Каталог устройств текущей системы.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceCatalog {
    /// Короткое имя платформы: `linux`, `win32`, `darwin`.
    pub platform: String,
    /// Микрофоны.
    pub inputs: Vec<DeviceInfo>,
    /// Источники системного звука.
    pub system: Vec<DeviceInfo>,
    /// Микрофон по умолчанию.
    pub default_input: Option<String>,
    /// Источник системного звука по умолчанию.
    pub default_system: Option<String>,
}

/// Что захватываем.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Микрофон.
    Microphone,
    /// Системный звук (что слышно из колонок).
    System,
}

/// Запрос на запись звука.
#[derive(Debug, Clone)]
pub struct CaptureRequest {
    /// Частота записи, Гц (16 000 — требование моделей распознавания).
    pub rate: u32,
    /// Микрофон: `None` — устройство по умолчанию.
    pub mic: Option<String>,
    /// Системный звук: `None` — не записывать.
    pub system: Option<String>,
    /// Куда писать сырой микрофон.
    pub mic_path: PathBuf,
    /// Куда писать сырой системный звук.
    pub system_path: PathBuf,
}

/// Один живой поток записи.
pub struct CaptureStream {
    /// Файл с сырыми сэмплами (моно f32 little-endian).
    pub path: PathBuf,
    level: Arc<LevelMeter>,
    stop: Arc<AtomicBool>,
    /// Сигнал завершения рабочего потока (для ожидания с таймаутом).
    done: Option<std::sync::mpsc::Receiver<()>>,
    worker: Option<JoinHandle<()>>,
}

impl CaptureStream {
    /// Создать поток из рабочего потока (платформенный бэкенд).
    ///
    /// Возвращает ошибку, если ОС не смогла стартовать поток — иначе пользователь
    /// узнал бы о пустой записи только на остановке.
    // На платформах без живого бэкенда (android/ios) вызовов нет — это норма.
    #[cfg_attr(
        not(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos",
            target_os = "android"
        )),
        allow(dead_code)
    )]
    pub(crate) fn spawn<F>(path: PathBuf, worker: F) -> Result<Self>
    where
        F: FnOnce(&Path, &Arc<LevelMeter>, &AtomicBool) + Send + 'static,
    {
        let level = LevelMeter::new();
        let stop = Arc::new(AtomicBool::new(false));
        let level_c = level.clone();
        let stop_c = stop.clone();
        let path_c = path.clone();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::Builder::new()
            .name("mr-capture".into())
            .spawn(move || {
                worker(&path_c, &level_c, &stop_c);
                let _ = tx.send(());
            })
            .map_err(|e| AudioError::Record(format!("не создан поток захвата: {e}")))?;
        Ok(Self {
            path,
            level,
            stop,
            done: Some(rx),
            worker: Some(handle),
        })
    }

    /// Текущий уровень 0..1 (с запасом ×3).
    pub fn level(&self) -> f32 {
        self.level.level_01()
    }

    /// Текущий пик 0..1.
    pub fn peak(&self) -> f32 {
        self.level.peak_01()
    }

    /// Установить флаг остановки без ожидания потока.
    pub fn signal_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Остановить поток и дождаться завершения рабочего потока.
    ///
    /// Ждём до трёх секунд: если система звука «заснула», чтение может
    /// блокировать. Но поток не отсоединяем: рано удалённый сырой файл
    /// стоил бы хвоста записи (данные дописываются до самого конца).
    pub fn stop(&mut self) {
        if self.stop.swap(true, Ordering::Relaxed) {
            return;
        }
        if let Some(rx) = self.done.take() {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(3));
        }
        // Присоединяемся даже после таймаута: файл читает этот же поток записи.
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Результат запуска записи: микрофон и/или системный звук.
#[derive(Default)]
pub struct CaptureSession {
    /// Поток микрофона.
    pub mic: Option<CaptureStream>,
    /// Поток системного звука.
    pub system: Option<CaptureStream>,
}

impl CaptureSession {
    /// Уровень микрофона 0..1.
    pub fn mic_level(&self) -> f32 {
        self.mic.as_ref().map(|s| s.level()).unwrap_or(0.0)
    }

    /// Уровень системного звука 0..1.
    pub fn system_level(&self) -> f32 {
        self.system.as_ref().map(|s| s.level()).unwrap_or(0.0)
    }

    /// Пик микрофона 0..1 — детектор тишины.
    pub fn mic_peak(&self) -> f32 {
        self.mic.as_ref().map(|s| s.peak()).unwrap_or(0.0)
    }

    /// Остановить все потоки.
    pub fn stop(&mut self) {
        if let Some(s) = self.mic.as_ref() {
            s.signal_stop();
        }
        if let Some(s) = self.system.as_ref() {
            s.signal_stop();
        }
        if let Some(mut s) = self.mic.take() {
            s.stop();
        }
        if let Some(mut s) = self.system.take() {
            s.stop();
        }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Каталог устройств текущей ОС.
pub fn devices() -> DeviceCatalog {
    #[cfg(target_os = "linux")]
    {
        linux::devices()
    }
    #[cfg(target_os = "windows")]
    {
        windows::devices()
    }
    #[cfg(target_os = "macos")]
    {
        macos::devices()
    }
    #[cfg(target_os = "android")]
    {
        android::devices()
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    )))]
    {
        DeviceCatalog {
            platform: std::env::consts::OS.to_string(),
            ..Default::default()
        }
    }
}

/// Начать запись звука согласно запросу.
///
/// Микрофон обязателен: без него запись не имеет смысла. Системный звук —
/// best effort: если устройство недоступно, запись идёт без него.
pub fn start_capture(req: &CaptureRequest) -> Result<CaptureSession> {
    let mut session = CaptureSession::default();
    let mic = start_stream(
        StreamKind::Microphone,
        req.mic.as_deref(),
        &req.mic_path,
        req.rate,
    )?;
    session.mic = Some(mic);
    if let Some(system) = req.system.as_deref() {
        if !system.is_empty() {
            match start_stream(StreamKind::System, Some(system), &req.system_path, req.rate) {
                Ok(s) => session.system = Some(s),
                Err(e) => {
                    // Системный звук — необязательный: пишем тишину в правый канал.
                    eprintln!("Системный звук недоступен: {e}");
                }
            }
        }
    }
    Ok(session)
}

/// Запустить один поток на текущей платформе.
fn start_stream(
    kind: StreamKind,
    device: Option<&str>,
    path: &Path,
    rate: u32,
) -> Result<CaptureStream> {
    #[cfg(target_os = "linux")]
    {
        linux::start_stream(kind, device, path, rate)
    }
    #[cfg(target_os = "windows")]
    {
        windows::start_stream(kind, device, path, rate)
    }
    #[cfg(target_os = "macos")]
    {
        macos::start_stream(kind, device, path, rate)
    }
    #[cfg(target_os = "android")]
    {
        android::start_stream(kind, device, path, rate)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos",
        target_os = "android"
    )))]
    {
        let _ = (kind, device, path, rate);
        Err(AudioError::Other(format!(
            "живой захват не поддерживается на {}",
            std::env::consts::OS
        )))
    }
}
