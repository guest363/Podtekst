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

type ForegroundHandler = Box<dyn Fn(bool, bool) + Send + Sync>;

static FOREGROUND_HANDLER: std::sync::OnceLock<std::sync::Mutex<Option<ForegroundHandler>>> =
    std::sync::OnceLock::new();

pub fn set_foreground_handler(handler: ForegroundHandler) {
    let slot = FOREGROUND_HANDLER.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(handler);
    }
}

pub(crate) fn foreground_service(processing: bool, start: bool) {
    let Some(slot) = FOREGROUND_HANDLER.get() else {
        return;
    };
    if let Ok(guard) = slot.lock() {
        if let Some(handler) = guard.as_ref() {
            handler(processing, start);
        }
    }
}

pub fn foreground_processing_start() {
    foreground_service(true, true);
}

pub fn foreground_processing_stop() {
    foreground_service(true, false);
}

pub use stream::{read_raw_f32, resample_to};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub muted: bool,
    pub system: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceCatalog {
    pub platform: String,
    pub inputs: Vec<DeviceInfo>,
    pub system: Vec<DeviceInfo>,
    pub default_input: Option<String>,
    pub default_system: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Microphone,
    System,
}

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    /// Частота записи, Гц (16 000 — требование моделей распознавания).
    pub rate: u32,
    pub mic: Option<String>,
    pub system: Option<String>,
    pub mic_path: PathBuf,
    pub system_path: PathBuf,
}

pub struct CaptureStream {
    pub path: PathBuf,
    level: Arc<LevelMeter>,
    stop: Arc<AtomicBool>,
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

    pub fn level(&self) -> f32 {
        self.level.level_01()
    }

    pub fn peak(&self) -> f32 {
        self.level.peak_01()
    }

    pub fn signal_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Остановить поток и дождаться завершения рабочего потока.
    ///
    /// Ждём до трёх секунд: если система звука «заснула», чтение может
    /// блокировать. Зависший поток отсоединяем, чтобы вызывающий под mutex
    /// состояния записи не ждал вечно. Цена: пока процесс жив, бэкенд может
    /// дописывать сырой файл, поэтому сразу читать его после таймаута нельзя.
    pub fn stop(&mut self) {
        // Не swap: signal_stop мог выставить флаг раньше, а join всё равно нужен.
        self.stop.store(true, Ordering::Relaxed);
        let Some(worker) = self.worker.take() else {
            return;
        };
        let Some(rx) = self.done.take() else {
            let _ = worker.join();
            return;
        };
        match rx.recv_timeout(std::time::Duration::from_secs(3)) {
            // Поток завершился или запаниковал: join не блокирует.
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                eprintln!("Поток захвата не завершился за 3 с: отсоединён");
            }
        }
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
pub struct CaptureSession {
    pub mic: Option<CaptureStream>,
    pub system: Option<CaptureStream>,
}

impl CaptureSession {
    pub fn mic_level(&self) -> f32 {
        self.mic.as_ref().map(|s| s.level()).unwrap_or(0.0)
    }

    pub fn system_level(&self) -> f32 {
        self.system.as_ref().map(|s| s.level()).unwrap_or(0.0)
    }

    pub fn mic_peak(&self) -> f32 {
        self.mic.as_ref().map(|s| s.peak()).unwrap_or(0.0)
    }

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

/// Разрешение на запись системного звука (на macOS — запись экрана).
pub fn system_audio_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::screen_recording_granted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
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

#[cfg(test)]
mod tests {
    use super::CaptureStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn stop_joins_worker_after_signal_stop() {
        let path = std::env::temp_dir().join(format!("mr-stop-{}.f32", std::process::id()));
        let finished = Arc::new(AtomicBool::new(false));
        let finished_c = finished.clone();
        let mut stream = CaptureStream::spawn(path, move |_, _, stop| {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            finished_c.store(true, Ordering::Relaxed);
        })
        .expect("поток захвата");
        stream.signal_stop();
        stream.stop();
        assert!(
            finished.load(Ordering::Relaxed),
            "stop должен дождаться рабочего потока"
        );
        // Повторная остановка (и Drop) не должны ничего ломать.
        stream.stop();
    }
}
