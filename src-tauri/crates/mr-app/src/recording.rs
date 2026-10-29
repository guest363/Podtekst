//! Состояние записи: старт/остановка захвата, имена файлов, авто-расшифровка.

use mr_audio::ChannelMode;
use serde_json::json;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::save_dir;

struct WavCheckpoint {
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl WavCheckpoint {
    fn start(
        out: &Path,
        mic_path: PathBuf,
        sys_path: Option<PathBuf>,
        mix: bool,
    ) -> Result<Arc<Self>, String> {
        let spec = hound::WavSpec {
            channels: if mix { 1 } else { 2 },
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(out, spec).map_err(|e| e.to_string())?;
        let checkpoint = Arc::new(Self {
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
        });
        let stop = checkpoint.stop.clone();
        let handle = std::thread::Builder::new()
            .name("mr-wav-checkpoint".to_string())
            .spawn(move || {
                let mut writer = CheckpointWriter {
                    writer,
                    mic_path,
                    sys_path,
                    mic_offset: 0,
                    sys_offset: 0,
                    mic_pending: Vec::new(),
                    sys_pending: Vec::new(),
                    mix,
                };
                loop {
                    for _ in 0..30 {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    if let Err(e) = writer.append_and_flush() {
                        eprintln!("Ошибка чекпоинта WAV: {e}");
                        break;
                    }
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
            })
            .map_err(|e| format!("не создан поток чекпоинтов WAV: {e}"))?;
        *checkpoint.worker.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(checkpoint)
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WavCheckpoint {
    fn drop(&mut self) {
        self.stop();
    }
}

struct CheckpointWriter {
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    mic_path: PathBuf,
    sys_path: Option<PathBuf>,
    mic_offset: u64,
    sys_offset: u64,
    mic_pending: Vec<f32>,
    sys_pending: Vec<f32>,
    mix: bool,
}

impl CheckpointWriter {
    fn append_and_flush(&mut self) -> Result<(), String> {
        self.mic_pending
            .extend(read_new_samples(&self.mic_path, &mut self.mic_offset)?);
        if let Some(path) = self.sys_path.as_ref() {
            self.sys_pending
                .extend(read_new_samples(path, &mut self.sys_offset)?);
        }
        let has_sys = self.sys_path.is_some();
        let count = if has_sys {
            self.mic_pending.len().min(self.sys_pending.len())
        } else {
            self.mic_pending.len()
        };
        for i in 0..count {
            let mic = self.mic_pending[i];
            if self.mix {
                let sample = if has_sys {
                    (mic + self.sys_pending[i]) * 0.5
                } else {
                    mic
                };
                self.writer
                    .write_sample(pcm(sample))
                    .map_err(|e| e.to_string())?;
            } else {
                self.writer
                    .write_sample(pcm(mic))
                    .map_err(|e| e.to_string())?;
                self.writer
                    .write_sample(pcm(self.sys_pending[i]))
                    .map_err(|e| e.to_string())?;
            }
        }
        self.mic_pending.drain(..count);
        self.sys_pending.drain(..count);
        // hound обновляет RIFF/data lengths и оставляет валидный WAV на этом месте.
        self.writer.flush().map_err(|e| e.to_string())
    }
}

fn read_new_samples(path: &Path, offset: &mut u64) -> Result<Vec<f32>, String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    file.seek(SeekFrom::Start(*offset))
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let complete = bytes.len() / 4 * 4;
    *offset += complete as u64;
    Ok(bytes[..complete]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect())
}

fn pcm(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

#[derive(Clone)]
pub(crate) struct RecordingState {
    pub(crate) is_recording: bool,
    pub(crate) start_time: u64,
    pub(crate) current_file: Option<String>,
    pub(crate) mode: String,
    /// Живой захват в `Arc`, чтобы `Clone` для `/api/status` работал.
    pub(crate) capture: Option<std::sync::Arc<Mutex<mr_audio::capture::CaptureSession>>>,
    checkpoint: Option<Arc<WavCheckpoint>>,
}

impl Default for RecordingState {
    fn default() -> Self {
        Self {
            is_recording: false,
            start_time: 0,
            current_file: None,
            mode: "split".to_string(),
            capture: None,
            checkpoint: None,
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
    let catalog = mr_audio::capture::devices();
    let has_system = catalog.default_system.is_some();
    // Android и платформы без второго канала (на macOS — без BlackHole) не
    // могут писать split: он дал бы канал из нулей и зря удвоил файл и VAD.
    let mode = if cfg!(target_os = "android") || !has_system {
        "mix"
    } else {
        mode
    };
    let dir = save_dir();
    let _ = std::fs::create_dir_all(&dir);
    let started = now_secs();
    let fname = recording_name(started, None);
    let out = unique_media_path(&dir, &fname);
    let fname = out
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(fname);
    crate::media_registry::mark_owned(&out);
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
        Err(e) => {
            crate::media_registry::unmark_owned(&out);
            return json!({"ok": false, "error": e.to_string()});
        }
    };
    let has_sys = session.system.is_some();
    // Если системный канал не открылся, честно пишем моно, а не стерео с нулями.
    let actual_mode = if has_sys { mode } else { "mix" };
    let checkpoint = match WavCheckpoint::start(
        &out,
        request.mic_path.clone(),
        has_sys.then(|| request.system_path.clone()),
        actual_mode == "mix",
    ) {
        Ok(checkpoint) => checkpoint,
        Err(e) => {
            let mut session = session;
            session.stop();
            crate::media_registry::unmark_owned(&out);
            return json!({"ok": false, "error": e});
        }
    };
    st.capture = Some(std::sync::Arc::new(Mutex::new(session)));
    st.checkpoint = Some(checkpoint);
    st.is_recording = true;
    notify_recording_state(true);
    st.start_time = started;
    st.current_file = Some(out.to_string_lossy().to_string());
    st.mode = actual_mode.to_string();
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
    if let Some(checkpoint) = st.checkpoint.take() {
        checkpoint.stop();
    }
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
                let hint = if cfg!(target_os = "macos") {
                    ": проверьте доступ к микрофону в «Системные настройки» → «Конфиденциальность и безопасность» → «Микрофон»"
                } else {
                    ""
                };
                return Err(format!("запись пустая (0 сэмплов){hint}"));
            }
            let mut mic = mic;
            let mut sys = sys;
            let max_len = mic.len().max(sys.len());
            mic.resize(max_len, 0.0);
            sys.resize(max_len, 0.0);
            let (l, r) = if st.mode == "mix" && sys_path.is_none() {
                (mic.clone(), Vec::new())
            } else {
                mr_audio::mix_channels(&mic, &sys, ChannelMode::parse(&st.mode))
            };
            if st.mode == "mix" {
                mr_audio::write_mono_wav(&out_path, &l, 16000).map_err(|e| e.to_string())?;
            } else {
                mr_audio::write_stereo_wav(&out_path, &l, &r, 16000).map_err(|e| e.to_string())?;
            }
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
                crate::media_registry::unmark_owned(&out_path);
                st.current_file = None;
                return json!({"ok": false, "error": format!("Не удалось склеить запись: {e}")});
            }
        }
    }
    // Фейк-режим тестов: файл уже записан в start, остаётся только переименовать.
    let old_path = PathBuf::from(&file);
    let (file, name) = finalize_recording_name(&file, started, ended);
    if old_path != Path::new(&file) {
        crate::media_registry::path_renamed(&old_path, Path::new(&file));
    }
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
        let sys = res.get("sys").and_then(|x| x.as_bool()).unwrap_or(false);
        let what = if sys {
            "🔴 Запись началась (микрофон + система)"
        } else {
            "🔴 Запись началась (микрофон)"
        };
        crate::desktop::notify_icon("Запись встреч", what, "media-record");
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
    //! Финализация активной записи по SIGINT/SIGTERM (P7): чекпоинт останавливается,
    //! затем из сырых дорожек собирается итоговый WAV и фиксируется имя записи.

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

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    fn raw_bytes(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn checkpoint_flush_keeps_wav_readable() {
        use std::io::Write as _;

        let dir = std::env::temp_dir().join(format!("mr-checkpoint-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mic_path = dir.join("mic.f32");
        let sys_path = dir.join("sys.f32");
        let wav_path = dir.join("recording.wav");
        std::fs::write(&mic_path, raw_bytes(&[0.5, -0.5])).unwrap();
        std::fs::write(&sys_path, raw_bytes(&[0.25, -0.25])).unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut checkpoint = CheckpointWriter {
            writer: hound::WavWriter::create(&wav_path, spec).unwrap(),
            mic_path: mic_path.clone(),
            sys_path: Some(sys_path.clone()),
            mic_offset: 0,
            sys_offset: 0,
            mic_pending: Vec::new(),
            sys_pending: Vec::new(),
            mix: false,
        };
        checkpoint.append_and_flush().unwrap();
        {
            let mut reader = hound::WavReader::open(&wav_path).unwrap();
            assert_eq!(reader.duration(), 2);
            assert_eq!(reader.spec().channels, 2);
            let samples = reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(samples, vec![16383, 8191, -16383, -8191]);
        }

        for (path, sample) in [(&mic_path, 0.75_f32), (&sys_path, 0.5_f32)] {
            let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
        checkpoint.append_and_flush().unwrap();
        {
            let mut reader = hound::WavReader::open(&wav_path).unwrap();
            assert_eq!(reader.duration(), 3);
            assert_eq!(reader.samples::<i16>().count(), 6);
        }
        drop(checkpoint);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
