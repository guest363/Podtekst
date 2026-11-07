use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::backend::{
    BackendError, BackendEvent, BackendInfo, CancelToken, GenerateRequest, Phase, SummaryBackend,
};
use super::config::{CONTEXT_TOKENS, MODEL_ID};
use super::runtime_locator::RuntimeInfo;
use helpers::{
    api_key_path, build_args, free_port, hide_window, map_ureq, new_token, prepend_lib_path,
    sanitize_output, spawn_stderr, token_hash, write_token,
};

mod helpers;

const IDLE_KILL: Duration = Duration::from_secs(120);
const HEALTH_WAIT: Duration = Duration::from_secs(10);

struct SidecarState {
    child: Child,
    port: u16,
    model_sha: String,
    token: String,
    token_path: PathBuf,
    stderr: Option<std::thread::JoinHandle<(u64, String)>>,
}

static SIDECAR: OnceLock<Mutex<Option<SidecarState>>> = OnceLock::new();
static LAST_USED: OnceLock<Mutex<Instant>> = OnceLock::new();
static IDLE_ARMED: AtomicBool = AtomicBool::new(false);

fn sidecar_cell() -> &'static Mutex<Option<SidecarState>> {
    SIDECAR.get_or_init(|| Mutex::new(None))
}

fn last_used() -> &'static Mutex<Instant> {
    LAST_USED.get_or_init(|| Mutex::new(Instant::now()))
}

pub fn touch() {
    *last_used().lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
}

fn health_ok(port: u16) -> bool {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(300))
        .timeout_read(Duration::from_millis(500))
        .build();
    agent
        .get(&format!("http://127.0.0.1:{port}/health"))
        .call()
        .map(|response| response.status() == 200)
        .unwrap_or(false)
}

fn wait_health(child: &mut Child, port: u16, cancel: &CancelToken) -> Result<(), BackendError> {
    let started = Instant::now();
    while started.elapsed() < HEALTH_WAIT {
        cancel.check()?;
        if health_ok(port) {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(BackendError::Load(format!(
                "exit {}",
                status.code().unwrap_or(-1)
            )));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err(BackendError::Load("health timeout".to_string()))
}

pub struct SidecarBackend {
    runtime: RuntimeInfo,
    model_sha: String,
    threads: u32,
}

impl SidecarBackend {
    pub fn new(runtime: &RuntimeInfo, model_sha: String, threads: u32) -> Self {
        Self {
            runtime: runtime.clone(),
            model_sha,
            threads,
        }
    }

    fn ensure_loaded(
        &self,
        req: &GenerateRequest,
        cancel: &CancelToken,
    ) -> Result<(), BackendError> {
        cancel.check()?;
        let mut cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
        let reuse = match cell.as_mut() {
            Some(state) => {
                let alive = state.child.try_wait().ok().flatten().is_none();
                alive && state.model_sha == self.model_sha && health_ok(state.port)
            }
            None => false,
        };
        if reuse {
            drop(cell);
            touch();
            return Ok(());
        }
        kill_state(cell.take());
        drop(cell);
        let port = free_port()?;
        let token = new_token();
        let token_path = api_key_path();
        write_token(&token_path, &token)?;
        let args = build_args(&req.model_path, port, &token_path, req.threads, req.n_ctx);
        let mut cmd = Command::new(&self.runtime.exe);
        cmd.args(&args)
            .current_dir(&self.runtime.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        prepend_lib_path(&mut cmd, &self.runtime.dir);
        hide_window(&mut cmd);
        let child = cmd.spawn().map_err(|e| BackendError::Load(e.to_string()))?;
        let pid = child.id();
        let start = super::process::start_time(pid).unwrap_or_default();
        let (mut child, stderr) = spawn_stderr(child);
        if let Err(e) = wait_health(&mut child, port, cancel) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr.join();
            let _ = std::fs::remove_file(&token_path);
            return Err(e);
        }
        if let Err(error) = super::memory::check_peak(pid) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr.join();
            let _ = std::fs::remove_file(&token_path);
            crate::note!("summary: post-load peak {} MiB", error_peak(&error));
            return Err(BackendError::Oom);
        }
        let record = super::process::PidRecord {
            pid,
            start_time: start,
            port,
            model_sha256: self.model_sha.clone(),
            token_hash: token_hash(&token),
        };
        let _ = super::process::write_record(&record);
        let mut cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
        kill_state(cell.take());
        *cell = Some(SidecarState {
            child,
            port,
            model_sha: self.model_sha.clone(),
            token,
            token_path,
            stderr: Some(stderr),
        });
        drop(cell);
        touch();
        Ok(())
    }

    fn run_generate(
        &self,
        req: &GenerateRequest,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> Result<String, BackendError> {
        self.ensure_loaded(req, cancel)?;
        let (port, token) = {
            let cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
            let state = cell.as_ref().ok_or(BackendError::Crashed)?;
            (state.port, state.token.clone())
        };
        let body = serde_json::json!({
            "model": MODEL_ID,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.prompt},
            ],
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "top_p": req.top_p,
            "repeat_penalty": req.repeat_penalty,
            "seed": req.seed,
            "stop": req.stop,
            "stream": true,
        });
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout_read(Duration::from_secs(120))
            .build();
        let response = agent
            .post(&format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {token}"))
            .send_string(&body.to_string())
            .map_err(map_ureq)?;
        on_event(BackendEvent {
            phase: Phase::Generate,
            tokens_done: 0,
            tokens_total: req.max_tokens,
        });
        let mut text = String::new();
        let mut done = 0u32;
        for line in BufReader::new(response.into_reader()).lines() {
            cancel.check()?;
            let Ok(line) = line else { break };
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload.trim() == "[DONE]" {
                break;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            if let Some(delta) = value
                .pointer("/choices/0/delta/content")
                .and_then(|v| v.as_str())
            {
                text.push_str(delta);
                done += 1;
                if done.is_multiple_of(8) {
                    on_event(BackendEvent {
                        phase: Phase::Generate,
                        tokens_done: done,
                        tokens_total: req.max_tokens,
                    });
                }
            }
        }
        on_event(BackendEvent {
            phase: Phase::Generate,
            tokens_done: done.max(req.max_tokens.min(done + 1)),
            tokens_total: req.max_tokens,
        });
        Ok(sanitize_output(&text))
    }
}

fn error_peak(error: &super::memory::MemoryError) -> u64 {
    match error {
        super::memory::MemoryError::Peak { peak_mb } => *peak_mb,
        _ => 0,
    }
}

impl SummaryBackend for SidecarBackend {
    fn probe(&mut self) -> Result<BackendInfo, BackendError> {
        Ok(BackendInfo {
            runtime: format!("llama.cpp/{}", self.runtime.version),
            version: self.runtime.version.clone(),
            commit: self.runtime.commit.clone(),
            model_id: MODEL_ID.to_string(),
            n_ctx: CONTEXT_TOKENS,
            threads: self.threads,
        })
    }

    fn generate(
        &mut self,
        req: &GenerateRequest,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> Result<String, BackendError> {
        self.run_generate(req, cancel, on_event)
    }

    fn shutdown(&mut self) {
        // Жизненным циклом владеет idle-монитор/`summary::shutdown`: job не
        // должен гасить загруженную модель между запросами.
    }
}

fn kill_state(state: Option<SidecarState>) {
    let Some(mut state) = state else {
        return;
    };
    let _ = state.child.kill();
    let _ = state.child.wait();
    if let Some(handle) = state.stderr.take() {
        let _ = handle.join();
    }
    let _ = std::fs::remove_file(&state.token_path);
}

pub fn kill_current() {
    let taken = {
        let mut cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
        cell.take()
    };
    kill_state(taken);
    super::process::remove_record();
}

pub fn loaded_model_sha() -> Option<String> {
    let mut cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = cell.as_mut() {
        if state.child.try_wait().ok().flatten().is_none() {
            return Some(state.model_sha.clone());
        }
    }
    kill_state(cell.take());
    None
}

/// Idle 120 с: после job процесс живёт до таймера, новое job сбрасывает его.
pub fn arm_idle_monitor() {
    if IDLE_ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(5));
        if super::is_active() {
            touch();
            continue;
        }
        let elapsed = last_used()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed();
        if elapsed >= IDLE_KILL && loaded_model_sha().is_some() {
            kill_current();
            touch();
        }
    });
}
