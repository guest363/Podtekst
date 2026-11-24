use mr_audio::ChannelMode;
use serde_json::json;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
                        crate::note!("Ошибка чекпоинта WAV: {e}");
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
        // Системный канал мог остановиться (AirPods): не замираем на min(),
        // иначе чекпоинт перестаёт писать и микрофон.
        let sys_peak = self
            .sys_pending
            .iter()
            .fold(0.0f32, |acc, &s| acc.max(s.abs()));
        let sys_live = has_sys && sys_peak >= 1e-5;
        let count = self.mic_pending.len().max(self.sys_pending.len());
        for i in 0..count {
            let mic = self.mic_pending.get(i).copied().unwrap_or(0.0);
            let sys = self.sys_pending.get(i).copied().unwrap_or(0.0);
            if self.mix {
                let sample = if sys_live { (mic + sys) * 0.5 } else { mic };
                self.writer
                    .write_sample(pcm(sample))
                    .map_err(|e| e.to_string())?;
            } else {
                self.writer
                    .write_sample(pcm(mic))
                    .map_err(|e| e.to_string())?;
                self.writer
                    .write_sample(pcm(sys))
                    .map_err(|e| e.to_string())?;
            }
        }
        self.mic_pending.drain(..count.min(self.mic_pending.len()));
        self.sys_pending.drain(..count.min(self.sys_pending.len()));
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

const RECOVER_CHUNK: usize = 64 * 1024;

/// Хвост убитой записи: сырые `.mic.f32`/`.sys.f32` — единственные полные
/// данные (чекпоинт WAV отстаёт до 30 с), поэтому на старте собираем WAV
/// из сырья и только после успеха удаляем его.
pub(crate) fn recover_interrupted(dir: &Path, min_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut recovered = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name
            .strip_prefix('.')
            .and_then(|n| n.strip_suffix(".mic.f32"))
        else {
            continue;
        };
        if older_than(&path, min_age) && recover_one(dir, stem).is_ok() {
            recovered += 1;
        }
    }
    recovered
}

fn older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(|modified| modified.elapsed().unwrap_or_default() >= age)
        .unwrap_or(true)
}

fn recover_one(dir: &Path, stem: &str) -> Result<(), String> {
    let mic_path = dir.join(format!(".{stem}.mic.f32"));
    let sys_path = dir.join(format!(".{stem}.sys.f32"));
    let has_sys = sys_path.is_file();
    let out = dir.join(format!("{stem}.wav"));
    // shortcut: имя остаётся стартовым (без времени окончания) — досчитать его
    // из числа сэмплов можно, когда появится спрос на точное имя.
    let tmp = dir.join(format!(".{stem}.recover.wav"));
    let spec = hound::WavSpec {
        channels: if has_sys { 2 } else { 1 },
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&tmp, spec).map_err(|e| e.to_string())?;
    let mut mic = std::io::BufReader::new(
        std::fs::File::open(&mic_path).map_err(|e| format!("микрофон: {e}"))?,
    );
    let mut sys = if has_sys {
        Some(std::io::BufReader::new(
            std::fs::File::open(&sys_path).map_err(|e| format!("система: {e}"))?,
        ))
    } else {
        None
    };
    let mut mic_block = Block::default();
    let mut sys_block = Block::default();
    let mut frames = 0u64;
    loop {
        let mic_count = mic_block.read(&mut mic)?;
        let sys_count = match sys.as_mut() {
            Some(reader) => sys_block.read(reader)?,
            None => 0,
        };
        let count = mic_count.max(sys_count);
        if count == 0 {
            break;
        }
        for index in 0..count {
            writer
                .write_sample(pcm(mic_block.samples.get(index).copied().unwrap_or(0.0)))
                .map_err(|e| e.to_string())?;
            if has_sys {
                writer
                    .write_sample(pcm(sys_block.samples.get(index).copied().unwrap_or(0.0)))
                    .map_err(|e| e.to_string())?;
            }
        }
        frames += count as u64;
    }
    let finished = writer.finalize().map_err(|e| e.to_string());
    if let Err(e) = finished {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if frames == 0 {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&mic_path);
        let _ = std::fs::remove_file(&sys_path);
        return Ok(());
    }
    if std::fs::rename(&tmp, &out).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err("не удалось заменить WAV".to_string());
    }
    crate::note!("Восстановлен хвост прерванной записи: {} с", frames / 16000);
    let _ = std::fs::remove_file(&mic_path);
    let _ = std::fs::remove_file(&sys_path);
    Ok(())
}

#[derive(Default)]
struct Block {
    bytes: Vec<u8>,
    samples: Vec<f32>,
}

impl Block {
    fn read(&mut self, reader: &mut impl Read) -> Result<usize, String> {
        self.bytes.resize(RECOVER_CHUNK * 4, 0);
        let mut filled = 0;
        while filled < self.bytes.len() {
            let read = reader
                .read(&mut self.bytes[filled..])
                .map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        let complete = filled / 4 * 4;
        self.samples.clear();
        self.samples.extend(
            self.bytes[..complete]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| f32::from_le_bytes(*chunk)),
        );
        Ok(complete / 4)
    }
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
    pub(crate) screen_session: Option<Arc<Mutex<Option<crate::screen::ScreenCaptureSession>>>>,
    pub(crate) session_id: Option<String>,
    /// Некритичные сбои старта: журнал и диагностика показывают их без падения записи.
    pub(crate) warnings: Vec<String>,
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
            screen_session: None,
            session_id: None,
            warnings: Vec::new(),
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
    RECORDING_ACTIVE.store(is_recording, Ordering::Relaxed);
    if let Some(slot) = RECORDING_STATE_CALLBACK.get() {
        if let Ok(g) = slot.lock() {
            if let Some(cb) = g.as_ref() {
                cb(is_recording);
            }
        }
    }
}

static RECORDING_ACTIVE: AtomicBool = AtomicBool::new(false);
static RECORDING_AUTO: AtomicBool = AtomicBool::new(false);
#[cfg(not(any(target_os = "android", target_os = "ios")))]
static LIFECYCLE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Факт записи без доступа к состоянию: замена бинарников ffmpeg не должна
/// трогать файлы под работающим ffmpeg.
pub(crate) fn is_active() -> bool {
    RECORDING_ACTIVE.load(Ordering::Relaxed)
}

pub(crate) fn is_auto_recording() -> bool {
    RECORDING_AUTO.load(Ordering::SeqCst)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) fn is_lifecycle_in_flight() -> bool {
    LIFECYCLE_IN_FLIGHT.load(Ordering::SeqCst)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) fn clear_auto_started() {
    RECORDING_AUTO.store(false, Ordering::SeqCst);
}

/// Пока идёт открытие устройств или финализация WAV, мьютекс состояния
/// свободен: `/api/status`, `/api/journal` и CLI не должны ждать десятки секунд.
fn lifecycle<R>(
    state: &Arc<Mutex<RecordingState>>,
    run: impl FnOnce(&mut RecordingState) -> R,
) -> R {
    static SERIAL: Mutex<()> = Mutex::new(());
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    LIFECYCLE_IN_FLIGHT.store(true, Ordering::SeqCst);
    let taken = std::mem::take(&mut *state.lock().unwrap_or_else(|e| e.into_inner()));
    let mut slot = StateSlot { state, taken };
    run(&mut slot.taken)
}

struct StateSlot<'a> {
    state: &'a Arc<Mutex<RecordingState>>,
    taken: RecordingState,
}

impl Drop for StateSlot<'_> {
    fn drop(&mut self) {
        let taken = std::mem::take(&mut self.taken);
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = taken;
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        LIFECYCLE_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn start_recording(
    state: &Arc<Mutex<RecordingState>>,
    mode: &str,
    screen: bool,
) -> serde_json::Value {
    lifecycle(state, |st| {
        let res = start_recording_inner(st, mode, screen);
        if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
            RECORDING_AUTO.store(false, Ordering::SeqCst);
            crate::auto_detect::on_manual_start();
        }
        res
    })
}

pub(crate) fn stop_recording(
    state: &Arc<Mutex<RecordingState>>,
    defer_autotranscribe: bool,
) -> serde_json::Value {
    lifecycle(state, |st| {
        let was_recording = st.is_recording;
        let res = stop_recording_inner(st, defer_autotranscribe);
        if was_recording && !st.is_recording {
            crate::auto_detect::on_manual_stop();
        }
        res
    })
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn new_session_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{millis:x}-{:x}", std::process::id())
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

pub(crate) fn recording_name(start: u64, end: Option<u64>) -> String {
    let date = local_date(start);
    match end {
        Some(e) => format!("{}-{}__{}.wav", local_clock(start), local_clock(e), date),
        None => format!("{}__{}.wav", local_clock(start), date),
    }
}

/// Свободный путь: не перезаписываем существующую запись при совпадении имени.
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

pub(crate) fn start_recording_inner(
    st: &mut RecordingState,
    mode: &str,
    record_screen: bool,
) -> serde_json::Value {
    if st.is_recording {
        crate::journal::error(None, "start", "already_recording", "Запись уже идёт");
        return json!({"ok": false, "error": "Запись уже идёт"});
    }
    // Запись выигрывает у итогов: отмена до создания сессии записи.
    crate::summary::cancel_for_recording();
    let session_id = new_session_id();
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
        st.warnings.clear();
        st.session_id = Some(session_id.clone());
        crate::journal::start(&session_id, mode, "Тестовый захват", None, false);
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
            let message = e.to_string();
            crate::journal::error(Some(&session_id), "capture", "capture_failed", &message);
            return json!({"ok": false, "error": message});
        }
    };
    // Аудио уже пишется: от этой точки считаем, насколько отстал старт видео.
    let audio_started = Instant::now();
    let has_sys = session.system.is_some();
    let sys_error = session.system_error.clone();
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
            crate::journal::error(Some(&session_id), "checkpoint", "checkpoint_failed", &e);
            return json!({"ok": false, "error": e});
        }
    };

    let mut screen_error = None;
    let screen_session = if record_screen {
        match crate::screen::ScreenCaptureSession::start(started, audio_started) {
            Ok(sess) => Some(std::sync::Arc::new(Mutex::new(Some(sess)))),
            Err(e) => {
                // Сбой захвата экрана не должен отнимать запись звука встречи.
                crate::note!("[SCREEN] Запись экрана не запустилась, пишу только звук: {e}");
                screen_error = Some(format!("Ошибка запуска записи экрана: {e}"));
                None
            }
        }
    } else {
        None
    };

    st.capture = Some(std::sync::Arc::new(Mutex::new(session)));
    st.checkpoint = Some(checkpoint);
    st.screen_session = screen_session;
    st.is_recording = true;
    notify_recording_state(true);
    st.start_time = started;
    st.current_file = Some(out.to_string_lossy().to_string());
    st.mode = actual_mode.to_string();
    st.session_id = Some(session_id.clone());
    st.warnings.clear();
    if let Some(e) = sys_error.as_ref() {
        st.warnings.push(format!("system: {e}"));
    }
    if let Some(e) = screen_error.as_ref() {
        st.warnings.push(format!("screen: {e}"));
    }
    let sys_label = if has_sys {
        catalog
            .system
            .iter()
            .find(|d| catalog.default_system.as_deref() == Some(d.id.as_str()))
            .map(|d| d.name.clone())
    } else {
        None
    };
    let screen_started = st.screen_session.is_some();
    crate::journal::start(
        &session_id,
        actual_mode,
        &mic_label,
        sys_label.as_deref(),
        screen_started,
    );
    json!({"ok": true, "file": out.to_string_lossy(), "filename": fname,
        "mic": mic_label, "mic_muted": muted,
        "sys": has_sys, "sys_error": sys_error,
        "screen_error": screen_error})
}

/// Сессия экрана, которую нужно остановить на любом пути выхода из финализации.
struct ScreenGuard(Option<Arc<Mutex<Option<crate::screen::ScreenCaptureSession>>>>);

impl ScreenGuard {
    fn take(&mut self) -> Option<crate::screen::ScreenCaptureSession> {
        let arc = self.0.take()?;
        let session = arc.lock().ok()?.take();
        session
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        if let Some(session) = self.take() {
            let _ = session.stop();
        }
    }
}

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

pub(crate) fn stop_recording_inner(
    st: &mut RecordingState,
    defer_autotranscribe: bool,
) -> serde_json::Value {
    if !st.is_recording {
        crate::journal::error(
            st.session_id.as_deref(),
            "stop",
            "not_recording",
            "Запись не активна",
        );
        return json!({"ok": false, "error": "Запись не активна"});
    }
    let session = st.session_id.clone();
    st.is_recording = false;
    RECORDING_AUTO.store(false, Ordering::SeqCst);
    notify_recording_state(false);
    let ended = now_secs();
    let started = st.start_time;
    let elapsed = ended.saturating_sub(started);
    let file = st.current_file.clone().unwrap_or_default();
    let out_path = PathBuf::from(&file);
    if let Some(checkpoint) = st.checkpoint.take() {
        checkpoint.stop();
    }
    // Экран снимаем до финализации: при любой ошибке ниже сессия должна быть
    // остановлена, иначе ffmpeg продолжает писать экран после сбоя (P7).
    let mut screen = ScreenGuard(st.screen_session.take());
    let mode = st.mode.clone();
    let mut peak: Option<f32> = None;
    let mut sys_peak: Option<f32> = None;
    let mut mic_missing = false;
    if let Some(capture) = st.capture.take() {
        let mut finalize = || -> std::result::Result<(f32, Vec<String>, bool), String> {
            let (mic_path, sys_path, session_peak) = {
                let mut session = capture.lock().map_err(|_| "захват занят".to_string())?;
                let mic = session.mic.as_ref().map(|s| s.path.clone());
                let sys = session.system.as_ref().map(|s| s.path.clone());
                let peak = session.mic_peak();
                session.stop();
                (mic, sys, peak)
            };
            // Ошибка чтения дорожки — не «тишина»: иначе WAV с нулями и `ok: true`.
            let (mic, mic_error) = match mic_path.as_ref() {
                Some(path) => match mr_audio::capture::read_raw_f32(path) {
                    Ok(samples) => (samples, None),
                    Err(e) => (Vec::new(), Some(format!("микрофон: {e}"))),
                },
                None => (Vec::new(), None),
            };
            let (sys, sys_error) = match sys_path.as_ref() {
                Some(path) => match mr_audio::capture::read_raw_f32(path) {
                    Ok(samples) => (samples, None),
                    Err(e) => (Vec::new(), Some(format!("система: {e}"))),
                },
                None => (Vec::new(), None),
            };
            let warnings: Vec<String> = [mic_error, sys_error].into_iter().flatten().collect();
            let mic_missing = mic_path.is_some() && mic.is_empty();
            if mic.is_empty() && sys.is_empty() {
                for p in [mic_path.as_ref(), sys_path.as_ref()].into_iter().flatten() {
                    let _ = std::fs::remove_file(p);
                }
                let hint = if cfg!(target_os = "macos") {
                    ": проверьте доступ к микрофону в «Системные настройки» → «Конфиденциальность и безопасность» → «Микрофон»"
                } else {
                    ""
                };
                if let Some(warning) = warnings.first() {
                    return Err(format!("дорожки не прочитаны ({warning}){hint}"));
                }
                return Err(format!("запись пустая (0 сэмплов){hint}"));
            }
            let mut mic = mic;
            let mut sys = sys;
            let max_len = mic.len().max(sys.len());
            mic.resize(max_len, 0.0);
            sys.resize(max_len, 0.0);
            if sys_path.is_some() {
                sys_peak = Some(sys.iter().fold(0.0f32, |acc, &s| acc.max(s.abs())));
            }
            let (l, r) = if mode == "mix" && sys_path.is_none() {
                (std::mem::take(&mut mic), Vec::new())
            } else {
                mr_audio::mix_channels(&mic, &sys, ChannelMode::parse(&mode))
            };
            // Финал пишем во временный файл рядом с целевым: сбой не обрезает
            // уже сохранённый WAV, а rename атомарно подменяет его целиком.
            let tmp_path = out_path.with_extension("tmp.wav");
            let written = if mode == "mix" {
                mr_audio::write_mono_wav(&tmp_path, &l, 16000)
            } else {
                mr_audio::write_stereo_wav(&tmp_path, &l, &r, 16000)
            };
            if let Err(e) = written.map_err(|e| e.to_string()) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e);
            }
            if let Err(e) = std::fs::rename(&tmp_path, &out_path) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.to_string());
            }
            for p in [mic_path.as_ref(), sys_path.as_ref()].into_iter().flatten() {
                let _ = std::fs::remove_file(p);
            }
            let true_peak = mic
                .iter()
                .chain(sys.iter())
                .fold(session_peak, |acc, &s| acc.max(s.abs()));
            Ok((true_peak, warnings, mic_missing))
        };
        match finalize() {
            Ok((p, warnings, missing)) => {
                peak = Some(p);
                mic_missing = missing;
                st.warnings.extend(warnings);
            }
            Err(e) => {
                crate::media_registry::unmark_owned(&out_path);
                st.current_file = None;
                crate::journal::error(
                    session.as_deref(),
                    "finalize",
                    "finalize_failed",
                    &format!("Не удалось склеить запись: {e}"),
                );
                st.session_id = None;
                return json!({"ok": false, "error": format!("Не удалось склеить запись: {e}")});
            }
        }
    }
    let old_path = PathBuf::from(&file);
    let (mut file, mut name) = finalize_recording_name(&file, started, ended);
    if old_path != Path::new(&file) {
        crate::media_registry::path_renamed(&old_path, Path::new(&file));
    }

    let mut is_video = false;
    if let Some(session) = screen.take() {
        let video_offset = session.offset_sec();
        match session.stop() {
            Ok(video_tmp) => {
                let final_video = Path::new(&file).with_extension("mp4");
                match crate::screen::mux_screen_and_audio(
                    &video_tmp,
                    Path::new(&file),
                    &final_video,
                    video_offset,
                ) {
                    Ok(()) => {
                        crate::media_registry::unmark_owned(Path::new(&file));
                        crate::media_registry::mark_owned(&final_video);
                        file = final_video.to_string_lossy().to_string();
                        name = final_video
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or(name);
                        is_video = true;
                    }
                    Err(e) => {
                        crate::note!("[SCREEN] Ошибка склейки видео с аудио: {e}");
                        st.warnings.push(format!("screen mux: {e}"));
                    }
                }
            }
            Err(e) => {
                crate::note!("[SCREEN] Ошибка остановки захвата экрана: {e}");
                st.warnings.push(format!("screen stop: {e}"));
            }
        }
    }

    st.current_file = Some(file.clone());
    let mut resp = json!({
        "ok": true,
        "file": file,
        "filename": name,
        "elapsed_sec": elapsed,
        "is_video": is_video,
        "mic_missing": mic_missing,
    });
    if let Some(p) = peak {
        // shortcut: порог −46 дБFS; тише — почти наверняка мьют/не тот источник, чинить выбором дефолта в pactl
        resp["silent"] = json!(p < 0.005);
        resp["peak"] = json!(p);
    }
    if let Some(p) = sys_peak {
        resp["sys_silent"] = json!(p < 0.001);
        resp["sys_peak"] = json!(p);
    }
    crate::journal::finish(
        session.as_deref().unwrap_or_default(),
        "ok",
        elapsed as f64,
        &st.mode,
        if st.mode == "mix" { 1 } else { 2 },
        Path::new(&file),
        Path::new(&file).is_file(),
        is_video,
        resp.get("silent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        resp.get("sys_silent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        &st.warnings,
    );
    st.session_id = None;
    if !defer_autotranscribe {
        maybe_autotranscribe(&file);
    }
    resp
}

pub(crate) fn maybe_autotranscribe(file: &str) {
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
            hotwords: crate::config::configured_hotwords(),
            ctc_enabled: crate::config::configured_ctc_enabled(),
        };
        let _ = crate::transcribe::spawn_transcription_worker(PathBuf::from(file), opts);
    }
    #[cfg(not(all(feature = "onnx", not(test))))]
    let _ = file;
}

fn start_recording_notified(st: &mut RecordingState, auto: bool) -> serde_json::Value {
    let mode = crate::configured_record_mode();
    let res = start_recording_inner(st, &mode, false);
    if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let effective_auto = auto
            && crate::config::configured_auto_detect_mode() == crate::config::AutoDetectMode::Auto;
        #[cfg(any(target_os = "android", target_os = "ios"))]
        let effective_auto = false;
        RECORDING_AUTO.store(effective_auto, Ordering::SeqCst);
        if !effective_auto {
            crate::auto_detect::on_manual_start();
        }
        let sys = res.get("sys").and_then(|x| x.as_bool()).unwrap_or(false);
        let what = match (effective_auto, sys) {
            (true, true) => "Автозапись началась (микрофон + система)",
            (true, false) => "Автозапись началась (микрофон)",
            (false, true) => "🔴 Запись началась (микрофон + система)",
            (false, false) => "🔴 Запись началась (микрофон)",
        };
        crate::desktop::notify_icon("Подтекст", what, "media-record");
    } else if !auto {
        let err = res
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("Ошибка начала записи");
        crate::desktop::notify_icon("Подтекст", &format!("❌ {err}"), "dialog-error");
    }
    res
}

fn stop_recording_notified(st: &mut RecordingState, auto: bool) -> serde_json::Value {
    let was_recording = st.is_recording;
    let res = stop_recording_inner(st, false);
    if !auto && was_recording && !st.is_recording {
        crate::auto_detect::on_manual_stop();
    }
    if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        let fname = res
            .get("filename")
            .and_then(|x| x.as_str())
            .unwrap_or("Запись сохранена");
        crate::desktop::notify_icon(
            "Подтекст",
            &format!("⏹ Запись сохранена:\n{fname}"),
            "document-save",
        );
    } else if !auto {
        let err = res
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("Ошибка остановки записи");
        crate::desktop::notify_icon("Подтекст", &format!("❌ {err}"), "dialog-error");
    }
    res
}

pub(crate) fn start_recording_with_notify(state: &Arc<Mutex<RecordingState>>) -> serde_json::Value {
    lifecycle(state, |st| start_recording_notified(st, false))
}

pub(crate) fn stop_recording_with_notify(state: &Arc<Mutex<RecordingState>>) -> serde_json::Value {
    lifecycle(state, |st| stop_recording_notified(st, false))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) fn start_recording_auto(state: &Arc<Mutex<RecordingState>>) -> serde_json::Value {
    lifecycle(state, |st| {
        if st.is_recording
            || crate::config::configured_auto_detect_mode() != crate::config::AutoDetectMode::Auto
        {
            return json!({"ok": false, "aborted": true});
        }
        start_recording_notified(st, true)
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) fn stop_recording_auto(state: &Arc<Mutex<RecordingState>>) -> serde_json::Value {
    lifecycle(state, |st| {
        if !st.is_recording || !is_auto_recording() {
            return json!({"ok": false, "error": "Автозапись не активна"});
        }
        stop_recording_notified(st, true)
    })
}

pub(crate) fn toggle_recording_with_notify(
    state: &Arc<Mutex<RecordingState>>,
) -> serde_json::Value {
    lifecycle(state, |st| {
        if st.is_recording {
            stop_recording_notified(st, false)
        } else {
            start_recording_notified(st, false)
        }
    })
}

#[cfg(unix)]
pub(crate) mod shutdown {
    //! Финализация активной записи по SIGINT/SIGTERM (P7): чекпоинт останавливается,
    //! затем из сырых дорожек собирается итоговый WAV и фиксируется имя записи.

    use std::sync::atomic::{AtomicBool, Ordering};

    static PENDING: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(_: libc::c_int) {
        // SAFETY: обработчик только выставляет atomic — это async-signal-safe.
        PENDING.store(true, Ordering::SeqCst);
    }

    pub fn install(state: std::sync::Arc<std::sync::Mutex<crate::recording::RecordingState>>) {
        // SAFETY: libc::signal получает указатель на extern "C" функцию с
        // сигнатурой sighandler_t; обработчик не обращается к общим данным.
        unsafe {
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
        }
        std::thread::spawn(move || loop {
            if PENDING.load(Ordering::SeqCst) {
                crate::note!("\nЗавершение по сигналу: финализирую запись…");
                crate::auto_detect::stop();
                let recording = state.lock().unwrap_or_else(|e| e.into_inner()).is_recording;
                if recording {
                    let _ = super::stop_recording(&state, false);
                }
                crate::summary::shutdown();
                crate::crash::mark_clean();
                crate::logging::flush();
                crate::logging::shutdown();
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

    #[test]
    fn lifecycle_keeps_state_lock_free_during_work() {
        let state = Arc::new(Mutex::new(RecordingState::default()));
        let probe = state.clone();
        let free = lifecycle(&state, |_st| probe.try_lock().is_ok());
        assert!(free, "мьютекс состояния занят на время финализации");
        assert!(state.lock().is_ok(), "состояние должно вернуться в мьютекс");
    }

    #[test]
    fn recovers_tail_of_killed_recording() {
        let dir = std::env::temp_dir().join(format!("mr-recover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mic = dir.join(".take.mic.f32");
        let sys = dir.join(".take.sys.f32");
        std::fs::write(&mic, raw_bytes(&[0.5, -0.5, 0.25])).unwrap();
        std::fs::write(&sys, raw_bytes(&[0.1, -0.1])).unwrap();
        // Чекпоинт успел записать только первый сэмпл — сырьё полнее.
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut stale = hound::WavWriter::create(dir.join("take.wav"), spec).unwrap();
            stale.write_sample(0i16).unwrap();
            stale.write_sample(0i16).unwrap();
            stale.finalize().unwrap();
        }

        let recovered = recover_interrupted(&dir, Duration::ZERO);
        assert_eq!(recovered, 1);
        assert!(
            !mic.exists() && !sys.exists(),
            "сырьё удаляется после успеха"
        );

        let mut reader = hound::WavReader::open(dir.join("take.wav")).unwrap();
        assert_eq!(reader.spec().channels, 2);
        let samples: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>().unwrap();
        // Три кадра: микрофон длиннее, системный канал добит нулями.
        assert_eq!(samples.len(), 6, "кадры: {samples:?}");
        assert_eq!(samples[0], pcm(0.5));
        assert_eq!(samples[5], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_raw_files_are_not_recovered() {
        let dir = std::env::temp_dir().join(format!("mr-recover-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mic = dir.join(".live.mic.f32");
        std::fs::write(&mic, raw_bytes(&[0.5])).unwrap();
        assert_eq!(recover_interrupted(&dir, Duration::from_secs(15)), 0);
        assert!(mic.exists(), "свежее сырьё может принадлежать живой записи");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
