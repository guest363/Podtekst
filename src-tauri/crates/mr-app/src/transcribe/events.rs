//! SSE-шина событий транскрибации и загрузки моделей
//! и фоновый воркер. Транспорт отделён от пайплайна (P3).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::TranscribeOptions;

static EVENTS: OnceLock<Arc<Mutex<Vec<serde_json::Value>>>> = OnceLock::new();
static RUNNING: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();
static MODEL_RUNNING: OnceLock<Arc<Mutex<bool>>> = OnceLock::new();
/// Сколько событий вытеснено из буфера: курсоры SSE-клиентов абсолютные,
/// без учёта вытеснения reconnect по Last-Event-ID терял бы события.
static DRAINED: AtomicUsize = AtomicUsize::new(0);
/// Путь файла текущей расшифровки (для статуса UI; пусто, когда не выполняется).
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

/// Отдельный флаг скачивания моделей: иначе `finish_model_events` гасит общий флаг
/// посреди расшифровки и сервер обрывает её SSE-стрим.
fn model_running_flag() -> Arc<Mutex<bool>> {
    MODEL_RUNNING
        .get_or_init(|| Arc::new(Mutex::new(false)))
        .clone()
}

fn current_file_cell() -> Arc<Mutex<String>> {
    CURRENT_FILE
        .get_or_init(|| Arc::new(Mutex::new(String::new())))
        .clone()
}

/// Файл, который сейчас расшифровывается (пустая строка — ничего не расшифровывается).
pub fn transcribing_file() -> String {
    current_file_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// События после абсолютного курсора `since` (живёт через вытеснение буфера)
/// + новый абсолютный курсор + признак «что-то ещё выполняется».
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
        || *model_running_flag()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
    (fresh, total, running)
}

pub fn is_transcribe_running() -> bool {
    *running_flag().lock().unwrap_or_else(|e| e.into_inner())
}

/// Публичная отправка событий моделей/поддержки в общий SSE-поток.
pub fn emit_model_event(evt: serde_json::Value) {
    *model_running_flag()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = true;
    emit(evt);
}

/// Завершение потока событий моделей (вызывать после model-done).
pub fn finish_model_events() {
    *model_running_flag()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = false;
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
    }
}

/// Запускает транскрибацию в фоне; события забираются через `take_new_events`.
/// Возвращает false, если задача уже выполняется.
pub fn spawn_transcription_worker(audio_path: PathBuf, opts: TranscribeOptions) -> bool {
    let flag = running_flag();
    {
        let mut r = flag.lock().unwrap_or_else(|e| e.into_inner());
        if *r {
            return false;
        }
        *r = true;
    }
    {
        let cell = events();
        let mut store = cell.lock().unwrap_or_else(|e| e.into_inner());
        let cleared = store.len();
        store.clear();
        DRAINED.fetch_add(cleared, Ordering::Relaxed);
    }
    *current_file_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = audio_path.to_string_lossy().to_string();
    emit(serde_json::json!({"type": "stage", "pct": 0, "message": "Подготовка к расшифровке…"}));
    let guard = RunningGuard(flag);
    std::thread::spawn(move || {
        let _guard = guard;
        let result = super::transcribe_file(&audio_path, &opts, &|pct, msg| {
            emit(serde_json::json!({"type": "stage", "stage": "asr", "pct": pct, "message": msg}));
        });
        match result {
            Ok(rep) => {
                emit(
                    serde_json::json!({"type": "stage", "pct": 100, "message": "Расшифровка завершена"}),
                );
                emit(
                    serde_json::json!({"type": "done", "code": 0, "file": audio_path.to_string_lossy()}),
                );
                let _ = rep;
            }
            Err(e) => {
                emit(serde_json::json!({"type": "error", "message": e}));
            }
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Курсор абсолютный: после вытеснения начала буфера клиент, дочитавший до
    /// `total`, не должен ни потерять новые события, ни получить повтор старых.
    #[test]
    fn absolute_cursor_survives_drain() {
        for i in 0..MAX_EVENTS {
            emit(serde_json::json!({"type": "stage", "i": i}));
        }
        let (fresh, total, _) = take_new_events(0);
        assert_eq!(fresh.len(), MAX_EVENTS);
        assert_eq!(total, MAX_EVENTS);

        // Вытесняем 3 старых события и добавляем 2 новых.
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
}
