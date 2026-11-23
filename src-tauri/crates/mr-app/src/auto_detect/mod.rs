mod apps;
mod detector;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mr_audio::capture::{active_mic_clients, detection_capability, DetectionCapability};
use serde_json::{json, Value};

use self::apps::GENERIC_CALL;
use self::detector::{Detector, DetectorAction, TickContext};
use crate::config::{configured_auto_detect_mode, AutoDetectMode};
use crate::recording::RecordingState;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

struct Shared {
    detector: Mutex<Detector>,
    wake: Condvar,
    stop: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

fn shared() -> &'static Shared {
    SHARED.get_or_init(|| Shared {
        detector: Mutex::new(Detector::new()),
        wake: Condvar::new(),
        stop: AtomicBool::new(false),
        worker: Mutex::new(None),
    })
}

pub(crate) fn capability_str(cap: DetectionCapability) -> &'static str {
    match cap {
        DetectionCapability::Process => "process",
        DetectionCapability::DeviceOnly => "device_only",
        DetectionCapability::Unsupported => "unsupported",
    }
}

pub(crate) fn status_snapshot() -> Value {
    let cap = detection_capability();
    let prompt = SHARED
        .get()
        .and_then(|s| s.detector.lock().ok())
        .and_then(|d| d.prompt())
        .map(|p| json!({"id": p.id, "app": p.app}));
    json!({
        "supported": cap != DetectionCapability::Unsupported,
        "capability": capability_str(cap),
        "mode": configured_auto_detect_mode().as_str(),
        "auto_recording": crate::recording::is_auto_recording(),
        "prompt": prompt,
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) fn start(state: Arc<Mutex<RecordingState>>) {
    if detection_capability() == DetectionCapability::Unsupported {
        return;
    }
    let s = shared();
    let mut slot = s.worker.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return;
    }
    s.stop.store(false, Ordering::SeqCst);
    *slot = std::thread::Builder::new()
        .name("mr-auto-detect".to_string())
        .spawn(move || run_worker(state))
        .ok();
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) fn start(_state: Arc<Mutex<RecordingState>>) {}

pub(crate) fn stop() {
    let Some(s) = SHARED.get() else { return };
    {
        let _guard = s.detector.lock().unwrap_or_else(|e| e.into_inner());
        s.stop.store(true, Ordering::SeqCst);
        s.wake.notify_all();
    }
    if let Some(handle) = s.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = handle.join();
    }
}

pub(crate) fn notify_mode_changed() {
    let is_off = configured_auto_detect_mode() == AutoDetectMode::Off;
    if is_off {
        crate::recording::clear_auto_started();
    }
    let Some(s) = SHARED.get() else { return };
    let mut guard = s.detector.lock().unwrap_or_else(|e| e.into_inner());
    if is_off {
        guard.reset();
    }
    s.wake.notify_all();
}

pub(crate) fn on_manual_start() {
    let Some(s) = SHARED.get() else { return };
    s.detector
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .on_manual_start();
    s.wake.notify_all();
}

pub(crate) fn on_manual_stop() {
    let Some(s) = SHARED.get() else { return };
    s.detector
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .on_manual_stop();
    s.wake.notify_all();
}

pub(crate) fn dismiss_prompt() {
    let Some(s) = SHARED.get() else { return };
    s.detector
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .on_dismiss();
    s.wake.notify_all();
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn run_worker(state: Arc<Mutex<RecordingState>>) {
    let s = shared();
    let self_pid = std::process::id();
    loop {
        if s.stop.load(Ordering::SeqCst) {
            break;
        }
        let mode = configured_auto_detect_mode();
        if mode == AutoDetectMode::Off {
            let mut guard = s.detector.lock().unwrap_or_else(|e| e.into_inner());
            guard.reset();
            if s.stop.load(Ordering::SeqCst) || configured_auto_detect_mode() != AutoDetectMode::Off
            {
                continue;
            }
            drop(s.wake.wait(guard).unwrap_or_else(|e| e.into_inner()));
            continue;
        }
        let cap = detection_capability();
        let clients = active_mic_clients();
        let ctx = TickContext {
            is_recording: crate::recording::is_active(),
            is_auto_recording: crate::recording::is_auto_recording(),
            lifecycle_in_flight: crate::recording::is_lifecycle_in_flight(),
            self_pid,
        };
        let actions = {
            let mut det = s.detector.lock().unwrap_or_else(|e| e.into_inner());
            det.tick(Instant::now(), mode, cap, &clients, ctx)
        };
        for action in actions {
            execute_action(&state, s, action);
        }
        let guard = s.detector.lock().unwrap_or_else(|e| e.into_inner());
        if !s.stop.load(Ordering::SeqCst) {
            drop(
                s.wake
                    .wait_timeout(guard, POLL_INTERVAL)
                    .unwrap_or_else(|e| e.into_inner()),
            );
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn execute_action(state: &Arc<Mutex<RecordingState>>, s: &Shared, action: DetectorAction) {
    match action {
        DetectorAction::NotifyCall { slug, label } => {
            crate::note!("ev=auto_detect.prompt app={slug}");
            let hotkey = if cfg!(target_os = "macos") {
                "Cmd+Shift+R"
            } else {
                "Ctrl+Shift+R"
            };
            let body = if label == GENERIC_CALL.label {
                format!("Обнаружен звонок. Откройте Подтекст или нажмите {hotkey} для записи")
            } else {
                format!(
                    "Обнаружен звонок ({label}). Откройте Подтекст или нажмите {hotkey} для записи"
                )
            };
            crate::desktop::notify_icon("Подтекст", &body, "audio-input-microphone");
        }
        DetectorAction::StartAutoRecording { slug } => {
            let res = crate::recording::start_recording_auto(state);
            if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                crate::note!("ev=auto_detect.start app={slug}");
            } else {
                crate::note!("ev=auto_detect.start_failed app={slug}");
                s.detector
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .on_auto_start_failed();
            }
        }
        DetectorAction::StopAutoRecording { slug } => {
            crate::note!("ev=auto_detect.stop app={slug}");
            let _ = crate::recording::stop_recording_auto(state);
        }
    }
}
