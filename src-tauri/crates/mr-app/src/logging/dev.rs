//! Dev-sink: неблокирующий файл, кольцо последних строк и артефакты запуска.
//! Компилируется только при `debug_assertions`; в release модуля нет.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

pub(crate) const MARKER: &str = "PODTEKST_DEV_LOGS_V1";

const QUEUE_CAPACITY: usize = 8192;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const DROPPED_REPORT: Duration = Duration::from_secs(5);
const DEFAULT_RING: usize = 2048;
const DEFAULT_KEEP_RUNS: usize = 20;
const DEFAULT_MAX_MB: u64 = 64;
const RUNS_MAX_BYTES: u64 = 200 * 1024 * 1024;
const RUNS_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const MAX_DEV_LOGS: usize = 20;
const MAX_DEV_LOG_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Jsonl,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathMode {
    Redacted,
    Full,
}

#[derive(Clone)]
struct Filter {
    default: LevelFilter,
    targets: Vec<(String, LevelFilter)>,
    raw: String,
}

impl Filter {
    fn parse(raw: &str) -> Self {
        let mut filter = Self {
            default: LevelFilter::Info,
            targets: Vec::new(),
            raw: raw.to_string(),
        };
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match token.split_once('=') {
                Some((target, level)) => {
                    if let Some(level) = parse_level(level.trim()) {
                        filter.targets.push((target.trim().to_string(), level));
                    }
                }
                None => {
                    if let Some(level) = parse_level(token) {
                        filter.default = level;
                    }
                }
            }
        }
        filter
    }

    fn enabled(&self, target: &str, level: Level) -> bool {
        let filter = self
            .targets
            .iter()
            .rev()
            .find(|(name, _)| target == name || target.starts_with(&format!("{name}.")))
            .map(|(_, level)| *level)
            .unwrap_or(self.default);
        filter >= level.to_level_filter()
    }

    fn max(&self) -> LevelFilter {
        self.targets
            .iter()
            .map(|(_, level)| *level)
            .chain(std::iter::once(self.default))
            .max()
            .unwrap_or(LevelFilter::Off)
    }
}

fn parse_level(raw: &str) -> Option<LevelFilter> {
    match raw.to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct DevConfig {
    format: Format,
    paths: PathMode,
    ring_cap: usize,
}

impl DevConfig {
    fn from_env() -> Self {
        Self {
            format: match std::env::var("MR_LOG_FORMAT").as_deref() {
                Ok("jsonl") => Format::Jsonl,
                _ => Format::Text,
            },
            paths: path_mode(),
            ring_cap: env_usize("MR_DIAG_RING", DEFAULT_RING).clamp(16, 65_536),
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn path_mode() -> PathMode {
    match std::env::var("MR_LOG_PATHS").as_deref() {
        Ok("full") => PathMode::Full,
        _ => PathMode::Redacted,
    }
}

fn dump_mode() -> Option<bool> {
    // None — выключено, Some(full) — полный дамп, Some(false) — обычный.
    match std::env::var("MR_DIAG_DUMP").as_deref() {
        Ok("0") | Ok("off") => None,
        Ok("full") => Some(true),
        _ => Some(false),
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn escape_value(value: &str) -> String {
    if value
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    {
        format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        )
    } else {
        value.to_string()
    }
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r', '\t'], " ")
}

fn timestamp() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

struct KvCollector {
    fields: Vec<(String, serde_json::Value)>,
}

impl<'kvs> log::kv::VisitSource<'kvs> for KvCollector {
    fn visit_pair(
        &mut self,
        key: log::kv::Key<'kvs>,
        value: log::kv::Value<'kvs>,
    ) -> Result<(), log::kv::Error> {
        self.fields.push((key.to_string(), json_value(value)));
        Ok(())
    }
}

fn display_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn split_event(args: &str) -> (Option<&str>, &str) {
    if let Some(rest) = args.strip_prefix("ev=") {
        match rest.split_once(char::is_whitespace) {
            Some((event, message)) => (Some(event), message.trim_start()),
            None => (Some(rest), ""),
        }
    } else {
        (None, args)
    }
}

fn json_value(value: log::kv::Value<'_>) -> serde_json::Value {
    if let Some(v) = value.to_bool() {
        return v.into();
    }
    if let Some(v) = value.to_i64() {
        return v.into();
    }
    if let Some(v) = value.to_u64() {
        return v.into();
    }
    if let Some(v) = value.to_f64() {
        return serde_json::Number::from_f64(v)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| value.to_string().into());
    }
    value.to_string().into()
}

fn format_record(record: &Record<'_>, config: DevConfig) -> String {
    let args = one_line(&record.args().to_string());
    let (event, message) = split_event(&args);
    let mut collector = KvCollector { fields: Vec::new() };
    let _ = record.key_values().visit(&mut collector);
    let run = super::current_run_id().unwrap_or_default();
    let stage = super::current_stage();
    let level = record.level().to_string();
    match config.format {
        Format::Text => {
            let mut line = format!(
                "{} {:<5} {:<6} {}",
                timestamp(),
                level,
                record.target(),
                args
            );
            if !run.is_empty() {
                line.push_str(&format!(" run={}", escape_value(&run)));
            }
            if !stage.is_empty() {
                line.push_str(&format!(" stage={}", escape_value(&stage)));
            }
            for (key, value) in &collector.fields {
                line.push(' ');
                line.push_str(key);
                line.push('=');
                line.push_str(&escape_value(&display_value(value)));
            }
            line
        }
        Format::Jsonl => {
            let mut map = serde_json::Map::new();
            map.insert("ts".into(), timestamp().into());
            map.insert("level".into(), level.to_ascii_lowercase().into());
            map.insert("target".into(), record.target().into());
            if let Some(event) = event {
                map.insert("ev".into(), event.into());
            }
            if !message.is_empty() {
                map.insert("msg".into(), message.into());
            }
            if !run.is_empty() {
                map.insert("run".into(), run.into());
            }
            if !stage.is_empty() {
                map.insert("stage".into(), stage.into());
            }
            for (key, value) in collector.fields {
                map.insert(key, value);
            }
            serde_json::Value::Object(map).to_string()
        }
    }
}

enum Msg {
    Line(String),
    Flush,
    FlushSync(SyncSender<()>),
    Shutdown,
}

struct DevLogger {
    sender: SyncSender<Msg>,
    filter: Filter,
    config: DevConfig,
    ring: Arc<Mutex<VecDeque<String>>>,
    dropped: Arc<AtomicU64>,
}

impl Log for DevLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.filter.enabled(metadata.target(), metadata.level())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut line = format_record(record, self.config);
        if self.config.paths == PathMode::Redacted {
            line = crate::privacy::redact_text(&line);
        }
        push_ring(&self.ring, self.config.ring_cap, line.clone());
        enqueue(&self.sender, line, &self.dropped);
    }

    fn flush(&self) {
        let _ = self.sender.try_send(Msg::Flush);
    }
}

fn push_ring(ring: &Mutex<VecDeque<String>>, cap: usize, line: String) {
    let mut guard = ring.lock().unwrap_or_else(|e| e.into_inner());
    if guard.len() >= cap {
        guard.pop_front();
    }
    guard.push_back(line);
}

fn enqueue(sender: &SyncSender<Msg>, line: String, dropped: &AtomicU64) {
    match sender.try_send(Msg::Line(line)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

fn dropped_line(format: Format, count: u64) -> String {
    match format {
        Format::Text => format!(
            "{} {:<5} {:<6} ev=diag.dropped count={count}",
            timestamp(),
            "WARN",
            "diag"
        ),
        Format::Jsonl => serde_json::json!({
            "ts": timestamp(),
            "level": "warn",
            "target": "diag",
            "ev": "diag.dropped",
            "count": count,
        })
        .to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn writer_loop(
    receiver: Receiver<Msg>,
    mut file: std::fs::File,
    ring: Arc<Mutex<VecDeque<String>>>,
    dropped: Arc<AtomicU64>,
    config: DevConfig,
) {
    let mut reported = 0u64;
    let mut last_report = std::time::Instant::now();
    let flush = |file: &mut std::fs::File| {
        let _ = file.flush();
    };
    loop {
        match receiver.recv_timeout(FLUSH_INTERVAL) {
            Ok(Msg::Line(line)) => {
                let _ = writeln!(file, "{line}");
            }
            Ok(Msg::Flush) => flush(&mut file),
            Ok(Msg::FlushSync(ack)) => {
                flush(&mut file);
                let _ = ack.send(());
            }
            Ok(Msg::Shutdown) => {
                while let Ok(Msg::Line(line)) = receiver.try_recv() {
                    let _ = writeln!(file, "{line}");
                }
                flush(&mut file);
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                flush(&mut file);
                let total = dropped.load(Ordering::Relaxed);
                if total > reported && last_report.elapsed() >= DROPPED_REPORT {
                    let line = dropped_line(config.format, total - reported);
                    push_ring(&ring, config.ring_cap, line.clone());
                    let _ = writeln!(file, "{line}");
                    reported = total;
                    last_report = std::time::Instant::now();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                flush(&mut file);
                return;
            }
        }
    }
}

struct State {
    sender: SyncSender<Msg>,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

static STATE: OnceLock<State> = OnceLock::new();
static RING: OnceLock<Arc<Mutex<VecDeque<String>>>> = OnceLock::new();
static INIT: Once = Once::new();
static ARTIFACT_BYTES: AtomicU64 = AtomicU64::new(0);
static ARTIFACT_TRUNCATED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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

fn runs_dir() -> PathBuf {
    logs_root().join("dev-runs")
}

fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

fn append_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = open_private(path)?;
    file.write_all(bytes)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

fn retain_logs(dir: &Path, keep: usize, max_bytes: u64) {
    let mut files: Vec<(PathBuf, SystemTime, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let meta = entry.metadata().ok()?;
            Some((path, meta.modified().ok()?, meta.len()))
        })
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let mut total = 0;
    for (index, (path, _, size)) in files.into_iter().enumerate() {
        total += size;
        if index >= keep || total > max_bytes {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn retain_runs(root: &Path, keep: usize, max_bytes: u64, max_age: Duration) {
    let mut dirs: Vec<(PathBuf, SystemTime, u64)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let size = dir_size(&path);
            Some((path, meta.modified().ok()?, size))
        })
        .collect();
    dirs.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let now = SystemTime::now();
    let mut total = 0;
    for (index, (path, modified, size)) in dirs.into_iter().enumerate() {
        total += size;
        let old = now
            .duration_since(modified)
            .map(|d| d > max_age)
            .unwrap_or(false);
        if index >= keep || total > max_bytes || old {
            let _ = std::fs::remove_dir_all(path);
        }
    }
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
        if filter.max() == LevelFilter::Off {
            return;
        }
        let dir = dev_logs_dir();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("dev-логи: не создана папка {}: {error}", dir.display());
            return;
        }
        retain_logs(&dir, MAX_DEV_LOGS, MAX_DEV_LOG_BYTES);
        let name = format!(
            "dev-{}-{}.log",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            std::process::id()
        );
        let path = dir.join(name);
        let file = match open_private(&path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("dev-логи: не открыт {}: {error}", path.display());
                return;
            }
        };
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let dropped = Arc::new(AtomicU64::new(0));
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
        let logger = DevLogger {
            sender,
            filter: filter.clone(),
            config,
            ring,
            dropped,
        };
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
    ARTIFACT_BYTES.store(0, Ordering::Relaxed);
    ARTIFACT_TRUNCATED.store(false, Ordering::Relaxed);
    let dir = runs_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        retain_runs(
            &dir,
            env_usize("MR_DIAG_KEEP_RUNS", DEFAULT_KEEP_RUNS).clamp(1, 1000),
            RUNS_MAX_BYTES,
            RUNS_MAX_AGE,
        );
        let run_dir = dir.join(&id);
        let _ = std::fs::create_dir_all(&run_dir);
        let _ = write_private(
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
    dump_mode().is_some()
}

pub(crate) fn dump_full() -> bool {
    dump_mode() == Some(true)
}

fn run_dir() -> Option<PathBuf> {
    let id = current_run_id()?;
    Some(runs_dir().join(id))
}

fn check_budget(bytes: usize, config_max: u64) -> bool {
    let used = ARTIFACT_BYTES.load(Ordering::Relaxed);
    if used + bytes as u64 > config_max {
        if !ARTIFACT_TRUNCATED.swap(true, Ordering::Relaxed) {
            log::warn!(
                target: "diag",
                limit_mb = config_max / (1024 * 1024),
                used_mb = used / (1024 * 1024);
                "ev=diag.artifact.truncated"
            );
        }
        return false;
    }
    ARTIFACT_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    true
}

pub(crate) fn write_artifact(relative: &str, bytes: &[u8]) -> bool {
    let Some(dir) = run_dir() else {
        return false;
    };
    if !check_budget(
        bytes.len(),
        env_u64("MR_DIAG_MAX_MB", DEFAULT_MAX_MB) * 1024 * 1024,
    ) {
        return false;
    }
    let path = dir.join(relative);
    if let Err(error) = write_private(&path, bytes) {
        log::warn!(
            target: "diag",
            file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            error = error.to_string();
            "ev=diag.artifact.error"
        );
        return false;
    }
    true
}

pub(crate) fn append_artifact(relative: &str, bytes: &[u8]) -> bool {
    let Some(dir) = run_dir() else {
        return false;
    };
    if !check_budget(
        bytes.len(),
        env_u64("MR_DIAG_MAX_MB", DEFAULT_MAX_MB) * 1024 * 1024,
    ) {
        return false;
    }
    let path = dir.join(relative);
    if let Err(error) = append_private(&path, bytes) {
        log::warn!(
            target: "diag",
            file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            error = error.to_string();
            "ev=diag.artifact.error"
        );
        return false;
    }
    true
}

pub(crate) fn write_manifest(value: &serde_json::Value) -> bool {
    let Some(dir) = run_dir() else {
        return false;
    };
    match serde_json::to_vec_pretty(value) {
        Ok(bytes) => {
            let path = dir.join("manifest.json");
            match write_private(&path, &bytes) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!(target: "run", error = error.to_string(); "ev=run.manifest_error");
                    false
                }
            }
        }
        Err(error) => {
            log::warn!(target: "run", error = error.to_string(); "ev=run.manifest_error");
            false
        }
    }
}

pub(crate) fn append_error(value: &serde_json::Value) {
    let Some(dir) = run_dir() else {
        return;
    };
    let Ok(line) = serde_json::to_string(value) else {
        return;
    };
    if let Err(error) = append_private(&dir.join("errors.jsonl"), format!("{line}\n").as_bytes()) {
        log::warn!(target: "run", error = error.to_string(); "ev=run.error_artifact");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-devlog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn filter_parses_default_and_targets() {
        let filter = Filter::parse("info,vad=trace,asr=debug");
        assert!(filter.enabled("run", Level::Info));
        assert!(!filter.enabled("run", Level::Debug));
        assert!(filter.enabled("vad", Level::Trace));
        assert!(filter.enabled("vad.window", Level::Trace));
        assert!(filter.enabled("asr", Level::Debug));
        assert!(!filter.enabled("asr", Level::Trace));
        assert!(!filter.enabled("json", Level::Debug));
        assert_eq!(filter.max(), LevelFilter::Trace);

        let off = Filter::parse("off,vad=debug");
        assert!(!off.enabled("run", Level::Error));
        assert!(off.enabled("vad", Level::Debug));
        assert_eq!(Filter::parse("nonsense").default, LevelFilter::Info);
    }

    #[test]
    fn escape_quotes_spaces_newlines_and_unicode() {
        assert_eq!(escape_value("plain"), "plain");
        assert_eq!(escape_value("два слова"), "\"два слова\"");
        assert_eq!(escape_value("a\"b"), "\"a\\\"b\"");
        assert_eq!(escape_value("a\nb"), "\"a\\nb\"");
        assert_eq!(escape_value("путь\\x"), "\"путь\\\\x\"");
    }

    #[test]
    fn ring_keeps_only_last_lines() {
        let ring = Mutex::new(VecDeque::new());
        for i in 0..5 {
            push_ring(&ring, 3, format!("line {i}"));
        }
        let guard = ring.lock().unwrap();
        assert_eq!(guard.len(), 3);
        assert_eq!(guard.front().map(String::as_str), Some("line 2"));
        assert_eq!(guard.back().map(String::as_str), Some("line 4"));
    }

    #[test]
    fn full_queue_increments_dropped_without_blocking() {
        let (sender, _receiver) = sync_channel(1);
        let dropped = AtomicU64::new(0);
        enqueue(&sender, "one".to_string(), &dropped);
        enqueue(&sender, "two".to_string(), &dropped);
        enqueue(&sender, "three".to_string(), &dropped);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn run_retention_keeps_newest_and_drops_old() {
        let dir = temp_dir("runs");
        for i in 0..5 {
            let run = dir.join(format!("run-{i}"));
            std::fs::create_dir_all(&run).unwrap();
            std::fs::write(run.join("data.bin"), vec![0u8; 10]).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        retain_runs(&dir, 2, 1024 * 1024, Duration::from_secs(3600));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(left, vec!["run-3", "run-4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn artifact_budget_reports_truncation_once() {
        ARTIFACT_BYTES.store(0, Ordering::Relaxed);
        ARTIFACT_TRUNCATED.store(false, Ordering::Relaxed);
        assert!(check_budget(10, 100));
        assert!(check_budget(80, 100));
        assert!(!check_budget(20, 100));
        assert!(ARTIFACT_TRUNCATED.load(Ordering::Relaxed));
        assert_eq!(ARTIFACT_BYTES.load(Ordering::Relaxed), 90);
    }

    #[test]
    fn run_id_is_stable_shape() {
        let id = new_run_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 3, "{id}");
        assert_eq!(parts[2].len(), 4);
        assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()));
    }
}
