use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInfo {
    pub runtime: String,
    pub version: String,
    pub commit: String,
    pub model_id: String,
    pub n_ctx: u32,
    pub threads: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Load,
    Prepare,
    Generate,
    Map,
    Reduce,
    Verify,
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub model_path: PathBuf,
    pub system: String,
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f64,
    pub top_p: f64,
    pub repeat_penalty: f64,
    pub seed: u64,
    pub n_ctx: u32,
    pub threads: u32,
    pub stop: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct BackendEvent {
    pub phase: Phase,
    pub tokens_done: u32,
    pub tokens_total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    Cancelled,
    Load(String),
    Oom,
    Crashed,
    Http(String),
}

pub trait SummaryBackend: Send {
    fn probe(&mut self) -> Result<BackendInfo, BackendError>;
    fn generate(
        &mut self,
        req: &GenerateRequest,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> Result<String, BackendError>;
    fn shutdown(&mut self);
    fn last_load_ms(&self) -> u64 {
        0
    }
}

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), BackendError> {
        if self.is_cancelled() {
            Err(BackendError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// `MR_SUMMARY_FAKE_BACKEND=1|model_load|invalid|thin|slow` — только debug:
/// контрактные тесты не тянут 2.4 ГБ модель и настоящий runtime.
#[cfg(debug_assertions)]
pub fn fake_mode() -> Option<&'static str> {
    let value = std::env::var("MR_SUMMARY_FAKE_BACKEND").ok()?;
    match value.trim() {
        "" | "0" => None,
        "1" | "ok" => Some("ok"),
        "model_load" => Some("model_load"),
        "invalid" => Some("invalid"),
        "thin" => Some("thin"),
        "thin_once" => Some("thin_once"),
        "flat_once" => Some("flat_once"),
        "no_decisions" => Some("no_decisions"),
        "reduce_invalid" => Some("reduce_invalid"),
        "nonce_echo" => Some("nonce_echo"),
        "map_injection" => Some("map_injection"),
        "slow" => Some("slow"),
        _ => None,
    }
}

#[cfg(not(debug_assertions))]
pub fn fake_mode() -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_token_is_shared() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
        assert_eq!(b.check(), Err(BackendError::Cancelled));
    }
}
