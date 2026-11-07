//! SSE-шина событий транскрибации и загрузки моделей
//! и фоновый воркер. Транспорт отделён от пайплайна (P3).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::TranscribeOptions;

#[derive(Clone, Debug)]
pub struct QueueTask {
    pub audio_path: PathBuf,
    pub opts: TranscribeOptions,
}

static QUEUE: OnceLock<Arc<Mutex<VecDeque<QueueTask>>>> = OnceLock::new();

fn queue_cell() -> Arc<Mutex<VecDeque<QueueTask>>> {
    QUEUE
        .get_or_init(|| Arc::new(Mutex::new(VecDeque::new())))
        .clone()
}

static EVENTS: OnceLock<Arc<Mutex<Vec<serde_json::Value>>>> = OnceLock::new();
static RUNNING: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();
/// Сколько событий вытеснено из буфера: курсоры SSE-клиентов абсолютные,
/// без учёта вытеснения reconnect по Last-Event-ID терял бы события.
static DRAINED: AtomicUsize = AtomicUsize::new(0);
static CURRENT_FILE: OnceLock<Arc<Mutex<String>>> = OnceLock::new();
/// Верхняя граница буфера SSE-событий: стрим живёт, пока открыт хоть один EventSource.
const MAX_EVENTS: usize = 2000;

fn events() -> Arc<Mutex<Vec<serde_json::Value>>> {
    EVENTS
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

fn running_flag() -> Arc<Mutex<bool>> {
    RUNNING.get_or_init(|| Arc::new(Mutex::new(false))).clone()
}

/// Счётчик активных скачиваний моделей: SSE-стрим живёт, пока хотя бы одна модель качается.
static MODEL_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Активный summary job (H10): выставляется синхронно до ответа `202`,
/// снимается после терминального события, чтобы SSE не закрылся между ними.
static SUMMARY_ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn current_file_cell() -> Arc<Mutex<String>> {
    CURRENT_FILE
        .get_or_init(|| Arc::new(Mutex::new(String::new())))
        .clone()
}

pub fn transcribing_file() -> String {
    current_file_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub fn take_new_events(since: usize) -> (Vec<serde_json::Value>, usize, bool) {
    let ev = events();
    let lock = ev.lock().unwrap_or_else(|e| e.into_inner());
    let drained = DRAINED.load(Ordering::Relaxed);
    let skip = since.saturating_sub(drained);
    let fresh = if skip >= lock.len() {
        Vec::new()
    } else {
        lock.iter().skip(skip).cloned().collect::<Vec<_>>()
    };
    let total = drained + lock.len();
    drop(lock);
    let running = *running_flag().lock().unwrap_or_else(|e| e.into_inner())
        || MODEL_ACTIVE.load(Ordering::Relaxed) > 0
        || SUMMARY_ACTIVE.load(Ordering::Relaxed) > 0;
    (fresh, total, running)
}

pub fn is_transcribe_running() -> bool {
    *running_flag().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn start_model_download() {
    MODEL_ACTIVE.fetch_add(1, Ordering::SeqCst);
}

pub fn start_summary_activity() {
    SUMMARY_ACTIVE.fetch_add(1, Ordering::SeqCst);
}

pub fn finish_summary_activity() {
    let mut cur = SUMMARY_ACTIVE.load(Ordering::SeqCst);
    while cur > 0 {
        match SUMMARY_ACTIVE.compare_exchange_weak(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

pub fn is_summary_active() -> bool {
    SUMMARY_ACTIVE.load(Ordering::SeqCst) > 0
}

/// Идёт расшифровка или в очереди есть задачи: summary-генерация запрещена.
pub fn has_pending_work() -> bool {
    if is_transcribe_running() {
        return true;
    }
    let q = queue_cell();
    let lock = q.lock().unwrap_or_else(|e| e.into_inner());
    !lock.is_empty()
}

pub fn emit_model_event(evt: serde_json::Value) {
    emit(evt);
}

pub fn finish_model_events() {
    let mut cur = MODEL_ACTIVE.load(Ordering::SeqCst);
    while cur > 0 {
        match MODEL_ACTIVE.compare_exchange_weak(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

pub(crate) fn emit(evt: serde_json::Value) {
    let store = events();
    let mut lock = store.lock().unwrap_or_else(|e| e.into_inner());
    lock.push(evt);
    let excess = lock.len().saturating_sub(MAX_EVENTS);
    if excess > 0 {
        lock.drain(..excess);
        DRAINED.fetch_add(excess, Ordering::Relaxed);
    }
}

pub(crate) fn emit_stage(pct: u8, message: &str) {
    emit(serde_json::json!({"type": "stage", "stage": "rust", "pct": pct, "message": message}));
}

/// RAII-сброс флага: даже паника внутри `transcribe_file` не оставляет
/// «расшифровка уже выполняется» навсегда.
struct RunningGuard(Arc<Mutex<bool>>);
impl Drop for RunningGuard {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = false;
        *current_file_cell()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = String::new();
        mr_audio::capture::foreground_processing_stop();
    }
}

pub fn get_queue_status() -> serde_json::Value {
    let running = is_transcribe_running();
    let current_file = transcribing_file();
    let q = queue_cell();
    let lock = q.lock().unwrap_or_else(|e| e.into_inner());
    let pending: Vec<serde_json::Value> = lock
        .iter()
        .enumerate()
        .map(|(i, t)| {
            serde_json::json!({
                "position": i + 2,
                "file": t.audio_path.to_string_lossy(),
                "name": t.audio_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                "engine": t.opts.engine,
            })
        })
        .collect();
    serde_json::json!({
        "running": running,
        "current_file": if running && !current_file.is_empty() { Some(current_file) } else { None },
        "queue": pending,
        "total": if running { pending.len() + 1 } else { pending.len() }
    })
}

fn emit_queue_event(q: &VecDeque<QueueTask>) {
    let items: Vec<serde_json::Value> = q
        .iter()
        .enumerate()
        .map(|(i, t)| {
            serde_json::json!({
                "position": i + 2,
                "file": t.audio_path.to_string_lossy(),
                "name": t.audio_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                "engine": t.opts.engine,
            })
        })
        .collect();
    emit(serde_json::json!({
        "type": "queue",
        "queue": items,
        "count": items.len(),
    }));
}

pub fn remove_from_queue(needle: &str) -> bool {
    let q = queue_cell();
    let mut lock = q.lock().unwrap_or_else(|e| e.into_inner());
    let before = lock.len();
    lock.retain(|t| {
        let p = t.audio_path.to_string_lossy();
        let n = t
            .audio_path
            .file_name()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_default();
        p != needle && n != needle
    });
    let removed = lock.len() < before;
    if removed {
        emit_queue_event(&lock);
    }
    removed
}

pub fn clear_queue() -> usize {
    let q = queue_cell();
    let mut lock = q.lock().unwrap_or_else(|e| e.into_inner());
    let len = lock.len();
    lock.clear();
    if len > 0 {
        emit_queue_event(&lock);
    }
    len
}

pub fn rename_in_queue(old: &Path, new: &Path) {
    let q = queue_cell();
    let mut lock = q.lock().unwrap_or_else(|e| e.into_inner());
    let mut changed = false;
    for task in lock.iter_mut() {
        if task.audio_path == old {
            task.audio_path = new.to_path_buf();
            changed = true;
        }
    }
    if changed {
        emit_queue_event(&lock);
    }
}

pub fn enqueue_transcription(audio_path: PathBuf, opts: TranscribeOptions) -> (bool, usize) {
    // Расшифровка выигрывает у итогов: отмена до выставления RUNNING, чтобы
    // UI не увидел одновременно «итоги идут» и «расшифровка идёт».
    crate::summary::cancel_for_transcription();
    let flag = running_flag();
    let mut r = flag.lock().unwrap_or_else(|e| e.into_inner());
    let cur_str = audio_path.to_string_lossy().to_string();

    if *r {
        if transcribing_file() == cur_str {
            return (false, 1);
        }
        let q = queue_cell();
        let mut lock = q.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = lock.iter().position(|t| t.audio_path == audio_path) {
            return (false, pos + 2);
        }
        lock.push_back(QueueTask { audio_path, opts });
        let pos = lock.len() + 1;
        emit_queue_event(&lock);
        return (false, pos);
    }

    *r = true;
    *current_file_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = cur_str.clone();
    drop(r);

    {
        let cell = events();
        let mut store = cell.lock().unwrap_or_else(|e| e.into_inner());
        let cleared = store.len();
        store.clear();
        DRAINED.fetch_add(cleared, Ordering::Relaxed);
    }

    mr_audio::capture::foreground_processing_start();
    emit(
        serde_json::json!({"type": "stage", "pct": 0, "message": "Подготовка к расшифровке…", "file": cur_str}),
    );

    let guard = RunningGuard(flag);
    std::thread::spawn(move || {
        let _guard = guard;
        let mut path = audio_path;
        let mut current_opts = opts;
        loop {
            let file_str = path.to_string_lossy().to_string();
            emit(
                serde_json::json!({"type": "stage", "pct": 0, "message": "Подготовка к расшифровке…", "file": file_str}),
            );
            let result = super::transcribe_file(&path, &current_opts, &|pct, msg| {
                emit(
                    serde_json::json!({"type": "stage", "stage": "asr", "pct": pct, "message": msg, "file": file_str}),
                );
            });
            match result {
                Ok(rep) => {
                    crate::journal::transcription_finish(&path, "ok", rep.duration_sec);
                    emit(
                        serde_json::json!({"type": "stage", "pct": 100, "message": "Расшифровка завершена", "file": file_str}),
                    );
                    emit(serde_json::json!({"type": "done", "code": 0, "file": file_str}));
                }
                Err(e) => {
                    crate::journal::transcription_finish(&path, "error", 0.0);
                    emit(serde_json::json!({"type": "error", "message": e, "file": file_str}));
                }
            }

            let next = {
                let q = queue_cell();
                let mut lock = q.lock().unwrap_or_else(|e| e.into_inner());
                let task = lock.pop_front();
                emit_queue_event(&lock);
                task
            };

            if let Some(next_task) = next {
                path = next_task.audio_path;
                current_opts = next_task.opts;
                *current_file_cell()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = path.to_string_lossy().to_string();
            } else {
                break;
            }
        }
    });

    (true, 1)
}

pub fn spawn_transcription_worker(audio_path: PathBuf, opts: TranscribeOptions) -> bool {
    let (started, _) = enqueue_transcription(audio_path, opts);
    started
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Курсор абсолютный: после вытеснения начала буфера клиент, дочитавший до
    /// `total`, не должен ни потерять новые события, ни получить повтор старых.
    #[test]
    fn absolute_cursor_survives_drain() {
        // Тест проверяет глобальный буфер: изолируем его от других тестов.
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        events().lock().unwrap_or_else(|e| e.into_inner()).clear();
        DRAINED.store(0, std::sync::atomic::Ordering::Relaxed);
        for i in 0..MAX_EVENTS {
            emit(serde_json::json!({"type": "stage", "i": i}));
        }
        let (fresh, total, _) = take_new_events(0);
        assert_eq!(fresh.len(), MAX_EVENTS);
        assert_eq!(total, MAX_EVENTS);

        emit(serde_json::json!({"type": "a"}));
        emit(serde_json::json!({"type": "b"}));
        emit(serde_json::json!({"type": "c"}));
        let (fresh, total, _) = take_new_events(total);
        assert_eq!(total, MAX_EVENTS + 3);
        assert_eq!(fresh.len(), 3);

        // Клиент, отставший на всю историю (курсор 0), получает что есть теперь —
        // без паники и без выхода за границы.
        let (fresh, total, _) = take_new_events(0);
        assert_eq!(fresh.len(), MAX_EVENTS);
        assert_eq!(total, MAX_EVENTS + 3);
        assert_eq!(fresh.last().unwrap()["type"], "c");
    }

    #[test]
    fn queue_operations_roundtrip() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_queue();
        let opts = TranscribeOptions::default();
        let q = queue_cell();
        q.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(QueueTask {
                audio_path: PathBuf::from("/tmp/test1.wav"),
                opts: opts.clone(),
            });
        q.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(QueueTask {
                audio_path: PathBuf::from("/tmp/test2.wav"),
                opts,
            });
        let status = get_queue_status();
        assert_eq!(status["queue"].as_array().unwrap().len(), 2);

        rename_in_queue(Path::new("/tmp/test1.wav"), Path::new("/tmp/test3.wav"));
        assert_eq!(
            get_queue_status()["queue"][0]["file"].as_str(),
            Some("/tmp/test3.wav")
        );

        assert!(remove_from_queue("test3.wav"));
        let status_after = get_queue_status();
        assert_eq!(status_after["queue"].as_array().unwrap().len(), 1);

        assert_eq!(clear_queue(), 1);
        let status_empty = get_queue_status();
        assert_eq!(status_empty["queue"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn enqueue_while_running_queues_and_dedupes() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_queue();
        let opts = TranscribeOptions::default();
        *running_flag().lock().unwrap_or_else(|e| e.into_inner()) = true;
        *current_file_cell()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = "/tmp/current.wav".to_string();

        assert_eq!(
            enqueue_transcription(PathBuf::from("/tmp/current.wav"), opts.clone()),
            (false, 1)
        );
        assert_eq!(
            enqueue_transcription(PathBuf::from("/tmp/next.wav"), opts.clone()),
            (false, 2)
        );
        assert_eq!(
            enqueue_transcription(PathBuf::from("/tmp/next.wav"), opts.clone()),
            (false, 2)
        );
        assert_eq!(
            enqueue_transcription(PathBuf::from("/tmp/last.wav"), opts),
            (false, 3)
        );

        let status = get_queue_status();
        assert_eq!(status["running"].as_bool(), Some(true));
        assert_eq!(status["current_file"].as_str(), Some("/tmp/current.wav"));
        assert_eq!(status["queue"].as_array().unwrap().len(), 2);

        clear_queue();
        *running_flag().lock().unwrap_or_else(|e| e.into_inner()) = false;
        *current_file_cell()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = String::new();
    }
}
