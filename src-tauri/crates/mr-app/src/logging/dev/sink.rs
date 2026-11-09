use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{Log, Metadata, Record};

use super::filter::{DevConfig, Filter};
use super::format::{dropped_line, format_record, PathMode};

pub(super) const QUEUE_CAPACITY: usize = 8192;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const DROPPED_REPORT: Duration = Duration::from_secs(5);

pub(super) enum Msg {
    Line(String),
    Flush,
    FlushSync(SyncSender<()>),
    Shutdown,
}

pub(super) struct DevLogger {
    sender: SyncSender<Msg>,
    filter: Filter,
    config: DevConfig,
    ring: Arc<Mutex<VecDeque<String>>>,
    dropped: Arc<AtomicU64>,
}

impl DevLogger {
    pub(super) fn new(
        sender: SyncSender<Msg>,
        filter: Filter,
        config: DevConfig,
        ring: Arc<Mutex<VecDeque<String>>>,
        dropped: Arc<AtomicU64>,
    ) -> Self {
        Self {
            sender,
            filter,
            config,
            ring,
            dropped,
        }
    }
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

pub(super) fn push_ring(ring: &Mutex<VecDeque<String>>, cap: usize, line: String) {
    let mut guard = ring.lock().unwrap_or_else(|e| e.into_inner());
    if guard.len() >= cap {
        guard.pop_front();
    }
    guard.push_back(line);
}

pub(super) fn enqueue(sender: &SyncSender<Msg>, line: String, dropped: &AtomicU64) {
    match sender.try_send(Msg::Line(line)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn writer_loop(
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
