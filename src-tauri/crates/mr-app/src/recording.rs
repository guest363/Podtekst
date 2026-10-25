//! Состояние записи: старт/остановка захвата, имена файлов, авто-расшифровка.

use mr_audio::ChannelMode;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::save_dir;

#[derive(Clone)]
pub(crate) struct RecordingState {
    pub(crate) is_recording: bool,
    pub(crate) start_time: u64,
    pub(crate) current_file: Option<String>,
    pub(crate) mode: String,
    /// Живой захват в `Arc`, чтобы `Clone` для `/api/status` работал.
    pub(crate) capture: Option<std::sync::Arc<Mutex<mr_audio::capture::CaptureSession>>>,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self {
            is_recording: false,
            start_time: 0,
            current_file: None,
            mode: "split".to_string(),
            capture: None,
        }
    }
}

pub(crate) type RecordingStateCallback = Box<dyn Fn(bool) + Send + Sync + 'static>;
static RECORDING_STATE_CALLBACK: std::sync::OnceLock<Mutex<Option<RecordingStateCallback>>> =
    std::sync::OnceLock::new();

#[allow(dead_code)]
pub(crate) fn set_recording_state_callback(cb: RecordingStateCallback) {
    let slot = RECORDING_STATE_CALLBACK.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        *g = Some(cb);
    }
}

pub(crate) fn notify_recording_state(is_recording: bool) {
    if let Some(slot) = RECORDING_STATE_CALLBACK.get() {
        if let Ok(g) = slot.lock() {
            if let Some(cb) = g.as_ref() {
                cb(is_recording);
            }
        }
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Разделитель часов и минут: двоеточие нельзя в имени файла на Windows.
fn clock_sep() -> char {
    if cfg!(windows) {
        '-'
    } else {
        ':'
    }
}

fn local_clock(secs: u64) -> String {
    let dt = chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|d| d.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now);
    format!("{}{}{}", dt.format("%H"), clock_sep(), dt.format("%M"))
}

fn local_date(secs: u64) -> String {
    let dt = chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|d| d.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now);
    dt.format("%d-%m-%Y").to_string()
}

/// `ЧЧ:ММ-ЧЧ:ММ__ДД-ММ-ГГГГ.wav`; пока запись идёт, известен только старт.
pub(crate) fn recording_name(start: u64, end: Option<u64>) -> String {
    let date = local_date(start);
    match end {
        Some(e) => format!("{}-{}__{}.wav", local_clock(start), local_clock(e), date),
        None => format!("{}__{}.wav", local_clock(start), date),
    }
}

/// Свободный путь: не перезаписываем существующую запись при совпадении имени.
/// Сохраняет исходное расширение файла (`.wav`, `.mp3` и т.д.).
pub(crate) fn unique_media_path(dir: &Path, fname: &str) -> PathBuf {
    let first = dir.join(fname);
    if !first.exists() {
        return first;
    }
    let p = Path::new(fname);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| fname.to_string());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 2..1000 {
        let cand = dir.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    first
}

pub(crate) fn start_recording_inner(st: &mut RecordingState, mode: &str) -> serde_json::Value {
    if st.is_recording {
        return json!({"ok": false, "error": "Запись уже идёт"});
    }
    let dir = save_dir();
    let _ = std::fs::create_dir_all(&dir);
    let started = now_secs();
    let fname = recording_name(started, None);
    let out = unique_media_path(&dir, &fname);
    let fname = out
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(fname);
    // Тесты и машины без аудио: явный фейк вместо молчаливой тишины.
    if std::env::var("MR_FAKE_CAPTURE").is_ok() {
        let _ = mr_audio::write_stereo_wav(&out, &vec![0.0; 16000], &vec![0.0; 16000], 16000);
        st.is_recording = true;
        notify_recording_state(true);
        st.start_time = now_secs();
        st.current_file = Some(out.to_string_lossy().to_string());
        st.mode = mode.to_string();
        return json!({"ok": true, "file": out.to_string_lossy(), "filename": fname});
    }
    let catalog = mr_audio::capture::devices();
    let mic_id = catalog.default_input.clone();
    let mic_dev = catalog
        .inputs
        .iter()
        .find(|d| Some(&d.id) == mic_id.as_ref())
        .or_else(|| catalog.inputs.first());
    let mic_label = mic_dev
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "Микрофон".to_string());
    let muted = mic_dev.map(|d| d.muted).unwrap_or(false);
    let system_id = catalog.default_system.clone();

    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "take".to_string());
    let mic_raw = dir.join(format!(".{stem}.mic.f32"));
    let sys_raw = dir.join(format!(".{stem}.sys.f32"));
    let _ = std::fs::remove_file(&mic_raw);
    let _ = std::fs::remove_file(&sys_raw);
    let request = mr_audio::capture::CaptureRequest {
        rate: 16000,
        mic: mic_id,
        system: system_id,
        mic_path: mic_raw,
        system_path: sys_raw,
    };
    let session = match mr_audio::capture::start_capture(&request) {
        Ok(session) => session,
        Err(e) => return json!({"ok": false, "error": e.to_string()}),
    };
    let has_sys = session.system.is_some();
    st.capture = Some(std::sync::Arc::new(Mutex::new(session)));
    st.is_recording = true;
    notify_recording_state(true);
    st.start_time = started;
    st.current_file = Some(out.to_string_lossy().to_string());
    st.mode = mode.to_string();
    json!({"ok": true, "file": out.to_string_lossy(), "filename": fname,
        "mic": mic_label, "mic_muted": muted,
        "sys": has_sys})
}

/// Дописывает в имя записи время окончания: `ЧЧ:ММ__ДАТА.wav` → `ЧЧ:ММ-ЧЧ:ММ__ДАТА.wav`.
fn finalize_recording_name(file: &str, start: u64, end: u64) -> (String, String) {
    let old = PathBuf::from(file);
    let dir = old.parent().unwrap_or_else(|| Path::new("."));
    let old_name = old
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let wanted = recording_name(start, Some(end));
    if wanted == old_name {
        return (file.to_string(), old_name);
    }
    let dest = unique_media_path(dir, &wanted);
    if std::fs::rename(&old, &dest).is_err() {
        return (file.to_string(), old_name);
    }
    let name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(wanted);
    (dest.to_string_lossy().to_string(), name)
}

pub(crate) fn stop_recording_inner(st: &mut RecordingState) -> serde_json::Value {
    if !st.is_recording {
        return json!({"ok": false, "error": "Запись не активна"});
    }
    st.is_recording = false;
    notify_recording_state(false);
    let ended = now_secs();
    let started = st.start_time;
    let elapsed = ended.saturating_sub(started);
    let file = st.current_file.clone().unwrap_or_default();
    // Останавливаем захват и склеиваем сырые дорожки в стерео WAV 16 кГц.
    let out_path = PathBuf::from(&file);
    let mut peak: Option<f32> = None;
    if let Some(capture) = st.capture.take() {
        let finalize = || -> std::result::Result<f32, String> {
            let (mic_path, sys_path, session_peak) = {
                let mut session = capture.lock().map_err(|_| "захват занят".to_string())?;
                let mic = session.mic.as_ref().map(|s| s.path.clone());
                let sys = session.system.as_ref().map(|s| s.path.clone());
                let peak = session.mic_peak();
                session.stop();
                (mic, sys, peak)
            };
            let mic = mic_path
                .as_ref()
                .and_then(|p| mr_audio::capture::read_raw_f32(p).ok())
                .unwrap_or_default();
            let sys = sys_path
                .as_ref()
                .and_then(|p| mr_audio::capture::read_raw_f32(p).ok())
                .unwrap_or_default();
            if mic.is_empty() && sys.is_empty() {
                for p in [mic_path.as_ref(), sys_path.as_ref()].into_iter().flatten() {
                    let _ = std::fs::remove_file(p);
                }
                return Err("запись пустая (0 сэмплов)".to_string());
            }
            let mut mic = mic;
            let mut sys = sys;
            let max_len = mic.len().max(sys.len());
            mic.resize(max_len, 0.0);
            sys.resize(max_len, 0.0);
            let (l, r) = mr_audio::mix_channels(&mic, &sys, ChannelMode::parse(&st.mode));
            mr_audio::write_stereo_wav(&out_path, &l, &r, 16000).map_err(|e| e.to_string())?;
            for p in [mic_path.as_ref(), sys_path.as_ref()].into_iter().flatten() {
                let _ = std::fs::remove_file(p);
            }
            let true_peak = mic
                .iter()
                .chain(sys.iter())
                .fold(session_peak, |acc, &s| acc.max(s.abs()));
            Ok(true_peak)
        };
        match finalize() {
            Ok(p) => peak = Some(p),
            Err(e) => {
                st.current_file = None;
                return json!({"ok": false, "error": format!("Не удалось склеить запись: {e}")});
            }
        }
    }
    // Фейк-режим тестов: файл уже записан в start, остаётся только переименовать.
    let (file, name) = finalize_recording_name(&file, started, ended);
    st.current_file = Some(file.clone());
    let mut resp = json!({"ok": true, "file": file, "filename": name, "elapsed_sec": elapsed});
    if let Some(p) = peak {
        // shortcut: порог −46 дБFS; тише — почти наверняка мьют/не тот источник, чинить выбором дефолта в pactl
        resp["silent"] = json!(p < 0.005);
        resp["peak"] = json!(p);
    }
    maybe_autotranscribe(&file);
    resp
}

/// Запускает расшифровку сразу после остановки записи (если включено в настройках).
fn maybe_autotranscribe(file: &str) {
    #[cfg(all(feature = "onnx", not(test)))]
    {
        if !crate::config::configured_auto_transcribe() || !Path::new(file).is_file() {
            return;
        }
        let opts = crate::transcribe::TranscribeOptions {
            speaker_left: "Я".to_string(),
            speaker_right: "Собеседник".to_string(),
            engine: "auto".to_string(),
            language: "auto".to_string(),
            min_speakers: 0,
            max_speakers: 0,
        };
        let _ = crate::transcribe::spawn_transcription_worker(PathBuf::from(file), opts);
    }
    #[cfg(not(all(feature = "onnx", not(test))))]
    let _ = file;
}

/// Запуск записи с показом системного уведомления на рабочем столе.
pub(crate) fn start_recording_with_notify(st: &mut RecordingState) -> serde_json::Value {
    let mode = crate::configured_record_mode();
    let res = start_recording_inner(st, &mode);
    if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        crate::desktop::notify_icon(
            "Запись встреч",
            "🔴 Запись началась (микрофон + система)",
            "media-record",
        );
    } else {
        let err = res
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("Ошибка начала записи");
        crate::desktop::notify_icon("Запись встреч", &format!("❌ {err}"), "dialog-error");
    }
    res
}

/// Остановка записи с показом системного уведомления на рабочем столе.
pub(crate) fn stop_recording_with_notify(st: &mut RecordingState) -> serde_json::Value {
    let res = stop_recording_inner(st);
    if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        let fname = res
            .get("filename")
            .and_then(|x| x.as_str())
            .unwrap_or("Запись сохранена");
        crate::desktop::notify_icon(
            "Запись встреч",
            &format!("⏹ Запись сохранена:\n{fname}"),
            "document-save",
        );
    } else {
        let err = res
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("Ошибка остановки записи");
        crate::desktop::notify_icon("Запись встреч", &format!("❌ {err}"), "dialog-error");
    }
    res
}

/// Переключение записи (старт / стоп) с системным уведомлением.
pub(crate) fn toggle_recording_with_notify(st: &mut RecordingState) -> &'static str {
    if st.is_recording {
        stop_recording_with_notify(st);
        "STOPPED"
    } else {
        start_recording_with_notify(st);
        "STARTED"
    }
}

#[cfg(unix)]
pub(crate) mod shutdown {
    //! Финализация активной записи по SIGINT/SIGTERM (P7): WAV собирается
    //! при остановке, поэтому жёсткое завершение теряло запись целиком.

    use std::sync::atomic::{AtomicBool, Ordering};

    static PENDING: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(_: libc::c_int) {
        // Только async-signal-safe операции: выставляем флаг.
        PENDING.store(true, Ordering::SeqCst);
    }

    /// Установить обработчики и поток-наблюдатель, который при сигнале
    /// останавливает запись и завершает процесс кодом 0.
    pub fn install(state: std::sync::Arc<std::sync::Mutex<crate::recording::RecordingState>>) {
        unsafe {
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        }
        std::thread::spawn(move || loop {
            if PENDING.load(Ordering::SeqCst) {
                eprintln!("\nЗавершение по сигналу: финализирую запись…");
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                if st.is_recording {
                    let _ = super::stop_recording_inner(&mut st);
                }
                std::process::exit(0);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
    }
}
