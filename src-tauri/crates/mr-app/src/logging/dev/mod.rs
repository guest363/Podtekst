mod artifacts;
mod filter;
mod format;
mod retention;
mod sink;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use filter::{env_usize, DevConfig, Filter};
use sink::{writer_loop, DevLogger, Msg};

pub(crate) use artifacts::{append_artifact, append_error, write_artifact, write_manifest};

pub(crate) const MARKER: &str = "PODTEKST_DEV_LOGS_V1";

struct State {
    sender: SyncSender<Msg>,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

static STATE: OnceLock<State> = OnceLock::new();
static RING: OnceLock<Arc<Mutex<VecDeque<String>>>> = OnceLock::new();
static INIT: Once = Once::new();

thread_local! {
    static RUN_ID: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static STAGE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn ring() -> Option<&'static Arc<Mutex<VecDeque<String>>>> {
    RING.get()
}

fn logs_root() -> PathBuf {
    crate::logging::logs_dir()
}

fn dev_logs_dir() -> PathBuf {
    logs_root().join("dev")
}

pub(super) fn runs_dir() -> PathBuf {
    logs_root().join("dev-runs")
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn new_run_id() -> String {
    let now = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let hash = fnv1a(&format!("{}-{nanos}", std::process::id()));
    format!("{now}-{:04x}", hash as u16)
}

pub(crate) fn init_if_enabled() {
    INIT.call_once(|| {
        let config = DevConfig::from_env();
        let filter = Filter::parse(&std::env::var("MR_LOG").unwrap_or_else(|_| "info".to_string()));
        if filter.max() == log::LevelFilter::Off {
            return;
        }
        let dir = dev_logs_dir();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("dev-логи: не создана папка {}: {error}", dir.display());
            return;
        }
        retention::retain_logs(&dir, retention::MAX_DEV_LOGS, retention::MAX_DEV_LOG_BYTES);
        let name = format!(
            "dev-{}-{}.log",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            std::process::id()
        );
        let path = dir.join(name);
        let file = match retention::open_private(&path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("dev-логи: не открыт {}: {error}", path.display());
                return;
            }
        };
        let (sender, receiver) = sync_channel(sink::QUEUE_CAPACITY);
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let writer = std::thread::Builder::new().name("dev-log".into()).spawn({
            let ring = ring.clone();
            let dropped = dropped.clone();
            move || writer_loop(receiver, file, ring, dropped, config)
        });
        let writer = match writer {
            Ok(writer) => writer,
            Err(error) => {
                eprintln!("dev-логи: поток-писатель не стартовал: {error}");
                return;
            }
        };
        let _ = RING.set(ring.clone());
        let _ = STATE.set(State {
            sender: sender.clone(),
            writer: Mutex::new(Some(writer)),
        });
        let logger = DevLogger::new(sender, filter.clone(), config, ring, dropped);
        log::set_max_level(filter.max());
        if let Err(error) = log::set_boxed_logger(Box::new(logger)) {
            eprintln!("dev-логи: logger уже установлен: {error}");
            if let Some(state) = STATE.get() {
                let _ = state.sender.send(Msg::Shutdown);
            }
            return;
        }
        log::info!(
            target: "run",
            marker = MARKER,
            filter = filter.raw,
            file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            "ev=diag.init"
        );
    });
}

pub(crate) fn begin_run() -> String {
    let id = new_run_id();
    enter_run(&id);
    enter_stage("run");
    artifacts::reset_budget();
    let dir = runs_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        retention::retain_runs(
            &dir,
            env_usize("MR_DIAG_KEEP_RUNS", filter::DEFAULT_KEEP_RUNS).clamp(1, 1000),
            retention::RUNS_MAX_BYTES,
            retention::RUNS_MAX_AGE,
        );
        let run_dir = dir.join(&id);
        let _ = std::fs::create_dir_all(&run_dir);
        let _ = retention::write_private(
            &run_dir.join("README.txt"),
            "Локальные данные отладочного запуска.\n\
             Не прикладывайте к issue без проверки: manifest.json хранит полный путь\n\
             к входному файлу, артефакты могут содержать фрагменты текста.\n"
                .as_bytes(),
        );
    }
    id
}

pub(crate) fn enter_run(id: &str) {
    RUN_ID.with(|cell| *cell.borrow_mut() = Some(id.to_string()));
}

pub(crate) fn enter_stage(stage: &str) {
    STAGE.with(|cell| *cell.borrow_mut() = Some(stage.to_string()));
}

pub(crate) fn current_run_id() -> Option<String> {
    RUN_ID.with(|cell| cell.borrow().clone())
}

pub(crate) fn current_stage() -> String {
    STAGE.with(|cell| cell.borrow().clone().unwrap_or_default())
}

pub(crate) fn end_run() {
    RUN_ID.with(|cell| *cell.borrow_mut() = None);
    STAGE.with(|cell| *cell.borrow_mut() = None);
}

pub(crate) fn recent_lines(count: usize) -> Vec<String> {
    let Some(ring) = ring() else {
        return Vec::new();
    };
    // Паника может случиться внутри Log::log, где кольцо уже залочено:
    // try_lock не должен превращать отчёт о падении в зависание.
    let guard = match ring.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return Vec::new(),
    };
    let skip = guard.len().saturating_sub(count);
    guard.iter().skip(skip).cloned().collect()
}

pub(crate) fn flush() {
    let Some(state) = STATE.get() else {
        return;
    };
    // Синхронный сброс: выход из процесса не должен обгонять writer.
    let (ack, wait) = sync_channel(0);
    if state.sender.try_send(Msg::FlushSync(ack)).is_ok() {
        let _ = wait.recv_timeout(Duration::from_secs(1));
    }
}

pub(crate) fn shutdown() {
    let Some(state) = STATE.get() else {
        return;
    };
    let _ = state.sender.send(Msg::Shutdown);
    if let Ok(mut guard) = state.writer.lock() {
        if let Some(handle) = guard.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn dev_logs_enabled() -> bool {
    STATE.get().is_some()
}

pub(crate) fn dump_enabled() -> bool {
    filter::dump_mode().is_some()
}

pub(crate) fn dump_full() -> bool {
    filter::dump_mode() == Some(true)
}
