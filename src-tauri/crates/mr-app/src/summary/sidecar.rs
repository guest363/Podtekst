use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
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
    api_key_path, build_args, hide_window, map_ureq, new_token, prepend_lib_path, reserve_port,
    sanitize_output, spawn_stderr, token_hash, write_token,
};

mod helpers;

const IDLE_KILL: Duration = Duration::from_secs(120);
/// Холодная загрузка 2.4 ГБ на занятой машине не укладывается в 10 с;
/// 30 с — запас, после которого сломанный runtime уже не ждём.
const HEALTH_WAIT: Duration = Duration::from_secs(30);
/// Жёсткий потолок SSE: даже бесконечный поток не съест память.
const STREAM_CEILING: usize = 4 * 1024 * 1024;
/// Байт SSE на один токен с запасом на UTF-8 и служебный JSON.
const STREAM_BYTES_PER_TOKEN: usize = 128;

struct SidecarState {
    child: Child,
    port: u16,
    model_sha: String,
    gpu_layers: u32,
    gpu_device: Option<u32>,
    gpu_backend: Option<PathBuf>,
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

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(300))
        .timeout_read(Duration::from_millis(1000))
        .build()
}

fn auth(token: &str) -> String {
    format!("Bearer {token}")
}

fn health_ok(port: u16, token: &str) -> bool {
    agent()
        .get(&format!("http://127.0.0.1:{port}/health"))
        .set("Authorization", &auth(token))
        .call()
        .map(|response| response.status() == 200)
        .unwrap_or(false)
}

/// `/props` подтверждает, что на порту именно наш alias, файл модели и
/// контекст; одного 200 от чужого локального слушателя недостаточно.
fn props_ok(port: u16, token: &str, model_path: &Path, n_ctx: u32) -> bool {
    let Ok(response) = agent()
        .get(&format!("http://127.0.0.1:{port}/props"))
        .set("Authorization", &auth(token))
        .call()
    else {
        return false;
    };
    let Ok(text) = response.into_string() else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.get("model_alias").and_then(|v| v.as_str()) == Some(MODEL_ID)
        && value.get("total_slots").and_then(|v| v.as_u64()) == Some(1)
        && value
            .pointer("/default_generation_settings/n_ctx")
            .and_then(|v| v.as_u64())
            == Some(n_ctx as u64)
        && same_model_path(value.get("model_path").and_then(|v| v.as_str()), model_path)
}

fn same_model_path(reported: Option<&str>, expected: &Path) -> bool {
    let Some(reported) = reported else {
        return false;
    };
    let reported = Path::new(reported);
    match (reported.canonicalize(), expected.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => reported.file_name() == expected.file_name(),
    }
}

fn wait_health(
    child: &mut Child,
    port: u16,
    token: &str,
    cancel: &CancelToken,
) -> Result<(), BackendError> {
    let started = Instant::now();
    while started.elapsed() < HEALTH_WAIT {
        cancel.check()?;
        if health_ok(port, token) {
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
    fallback: Option<RuntimeInfo>,
    model_sha: String,
    threads: u32,
    gpu_layers: u32,
    gpu_device: Option<u32>,
    gpu_backend: Option<PathBuf>,
    load_ms: u64,
}

impl SidecarBackend {
    pub fn new(
        runtime: &RuntimeInfo,
        model_sha: String,
        threads: u32,
        gpu_layers: u32,
        gpu_device: Option<u32>,
        gpu_backend: Option<PathBuf>,
        fallback: Option<RuntimeInfo>,
    ) -> Self {
        Self {
            runtime: runtime.clone(),
            fallback,
            model_sha,
            threads,
            gpu_layers,
            gpu_device,
            gpu_backend,
            load_ms: 0,
        }
    }

    fn ensure_loaded(
        &mut self,
        req: &GenerateRequest,
        cancel: &CancelToken,
    ) -> Result<(), BackendError> {
        match self.ensure_loaded_once(req, cancel) {
            Ok(()) => Ok(()),
            Err(error @ (BackendError::Load(_) | BackendError::Oom | BackendError::Crashed))
                if self.gpu_layers > 0 && self.fallback.is_some() =>
            {
                crate::note!("summary: GPU load failed, CPU fallback: {error:?}");
                self.runtime = self.fallback.take().expect("fallback checked");
                self.gpu_layers = 0;
                self.gpu_device = None;
                self.gpu_backend = None;
                self.load_ms = 0;
                self.ensure_loaded_once(req, cancel)
            }
            Err(error) => Err(error),
        }
    }

    fn ensure_loaded_once(
        &mut self,
        req: &GenerateRequest,
        cancel: &CancelToken,
    ) -> Result<(), BackendError> {
        cancel.check()?;
        let mut cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
        let reuse = match cell.as_mut() {
            Some(state) => {
                let alive = state.child.try_wait().ok().flatten().is_none();
                alive
                    && state.model_sha == self.model_sha
                    && state.gpu_layers == self.gpu_layers
                    && state.gpu_device == self.gpu_device
                    && state.gpu_backend == self.gpu_backend
                    && health_ok(state.port, &state.token)
                    && props_ok(state.port, &state.token, &req.model_path, req.n_ctx)
            }
            None => false,
        };
        if reuse {
            drop(cell);
            self.load_ms = 0;
            touch();
            return Ok(());
        }
        kill_state(cell.take());
        drop(cell);
        let port_guard = reserve_port()?;
        let port = port_guard.port();
        let token = new_token();
        let token_path = api_key_path();
        write_token(&token_path, &token)?;
        let args = build_args(
            &req.model_path,
            port,
            &token_path,
            req.threads,
            req.n_ctx,
            self.gpu_layers,
            self.gpu_device,
        );
        let mut cmd = Command::new(&self.runtime.exe);
        cmd.args(&args)
            .current_dir(&self.runtime.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        prepend_lib_path(&mut cmd, &self.runtime.dir, self.gpu_backend.as_deref());
        if let Some(backend) = self.gpu_backend.as_deref() {
            cmd.env("GGML_BACKEND_PATH", backend);
        }
        hide_window(&mut cmd);
        let load_started = Instant::now();
        port_guard.release();
        let child = cmd.spawn().map_err(|e| BackendError::Load(e.to_string()))?;
        let pid = child.id();
        let start = super::process::start_time(pid).unwrap_or_default();
        let (mut child, stderr) = spawn_stderr(child);
        if let Err(e) = wait_health(&mut child, port, &token, cancel) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr.join();
            let _ = std::fs::remove_file(&token_path);
            return Err(e);
        }
        if !props_ok(port, &token, &req.model_path, req.n_ctx) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr.join();
            let _ = std::fs::remove_file(&token_path);
            return Err(BackendError::Load("props mismatch".to_string()));
        }
        self.load_ms = load_started.elapsed().as_millis() as u64;
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
            gpu_layers: self.gpu_layers,
            gpu_device: self.gpu_device,
            gpu_backend: self.gpu_backend.clone(),
            token,
            token_path,
            stderr: Some(stderr),
        });
        drop(cell);
        touch();
        Ok(())
    }

    fn run_generate(
        &mut self,
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
            .set("Authorization", &auth(&token))
            .send_string(&body.to_string())
            .map_err(map_ureq)?;
        on_event(BackendEvent {
            phase: Phase::Generate,
            tokens_done: 0,
            tokens_total: req.max_tokens,
        });
        let cap = (req.max_tokens as usize)
            .saturating_mul(STREAM_BYTES_PER_TOKEN)
            .clamp(256 * 1024, STREAM_CEILING);
        let reader = BufReader::new(response.into_reader().take(cap as u64 + 1));
        let text = read_stream(reader, req.max_tokens, cap, cancel, on_event)?;
        Ok(sanitize_output(&text))
    }
}

fn read_stream<R: BufRead>(
    reader: R,
    max_tokens: u32,
    cap: usize,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(BackendEvent),
) -> Result<String, BackendError> {
    let mut text = String::new();
    let mut bytes = 0usize;
    let mut done = 0u32;
    let mut finished = false;
    for line in reader.lines() {
        cancel.check()?;
        let line = line.map_err(|_| BackendError::Crashed)?;
        bytes += line.len() + 1;
        if bytes > cap {
            return Err(BackendError::Crashed);
        }
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload.trim() == "[DONE]" {
            finished = true;
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
                    tokens_total: max_tokens,
                });
            }
        }
    }
    if !finished {
        return Err(BackendError::Crashed);
    }
    on_event(BackendEvent {
        phase: Phase::Generate,
        tokens_done: max_tokens,
        tokens_total: max_tokens,
    });
    Ok(text)
}

fn error_peak(error: &super::memory::MemoryError) -> u64 {
    match error {
        super::memory::MemoryError::Peak { peak_mb } => *peak_mb,
        _ => 0,
    }
}

impl SummaryBackend for SidecarBackend {
    fn probe(&mut self) -> Result<BackendInfo, BackendError> {
        let suffix = if self.gpu_layers > 0 { "-gpu" } else { "" };
        Ok(BackendInfo {
            runtime: format!("llama.cpp/{}{suffix}", self.runtime.version),
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
        match self.run_generate(req, cancel, on_event) {
            Ok(text) => Ok(text),
            Err(BackendError::Crashed) if self.gpu_layers > 0 && self.fallback.is_some() => {
                crate::note!("summary: GPU generation crashed, CPU retry");
                kill_current();
                self.runtime = self.fallback.take().expect("fallback checked");
                self.gpu_layers = 0;
                self.gpu_device = None;
                self.gpu_backend = None;
                self.load_ms = 0;
                self.run_generate(req, cancel, on_event)
            }
            Err(error) => Err(error),
        }
    }

    fn shutdown(&mut self) {
        // Жизненным циклом владеет idle-монитор/`summary::shutdown`: job не
        // должен гасить загруженную модель между запросами.
    }

    fn last_load_ms(&self) -> u64 {
        self.load_ms
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
    let dead = {
        let mut cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
        let alive = cell
            .as_mut()
            .is_some_and(|state| state.child.try_wait().ok().flatten().is_none());
        if alive {
            return cell.as_ref().map(|state| state.model_sha.clone());
        }
        cell.take()
    };
    kill_state(dead);
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

#[cfg(test)]
pub(super) fn install_test_child(program: &str, args: &[&str]) -> u32 {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("test child");
    let pid = child.id();
    let token_path = std::env::temp_dir().join(format!("mr-sidecar-test-token-{pid}"));
    let mut cell = sidecar_cell().lock().unwrap_or_else(|e| e.into_inner());
    *cell = Some(SidecarState {
        child,
        port: 1,
        model_sha: "test".to_string(),
        gpu_layers: 0,
        gpu_device: None,
        gpu_backend: None,
        token: "test".to_string(),
        token_path,
        stderr: None,
    });
    pid
}

#[cfg(test)]
pub(super) fn test_state_pid() -> Option<u32> {
    sidecar_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|state| state.child.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn serve(status: &'static str, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = socket.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes());
            request
        });
        (port, handle)
    }

    #[test]
    fn health_requires_and_sends_bearer() {
        let (port, request) = serve("200 OK", r#"{"status":"ok"}"#);
        assert!(health_ok(port, "secret"));
        let request = request.join().unwrap();
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer secret"),
            "{request}"
        );
        let (port, _) = serve("401 Unauthorized", "{}");
        assert!(!health_ok(port, "secret"));
    }

    #[test]
    fn props_mismatch_is_rejected() {
        let ok = concat!(
            r#"{"model_alias":"sum-qwen3-4b-instruct-2507-q4km","#,
            r#""model_path":"/models/q.gguf","total_slots":1,"#,
            r#""default_generation_settings":{"n_ctx":8192}}"#
        );
        let (port, _) = serve("200 OK", ok);
        assert!(props_ok(port, "t", Path::new("/models/q.gguf"), 8192));
        let wrong_ctx = ok.replace("8192", "4096");
        let (port, _) = serve("200 OK", Box::leak(wrong_ctx.into_boxed_str()));
        assert!(!props_ok(port, "t", Path::new("/models/q.gguf"), 8192));
        let wrong_model = ok.replace("sum-qwen3-4b-instruct-2507-q4km", "other");
        let (port, _) = serve("200 OK", Box::leak(wrong_model.into_boxed_str()));
        assert!(!props_ok(port, "t", Path::new("/models/q.gguf"), 8192));
    }

    #[test]
    fn stream_without_done_marker_is_rejected() {
        let data = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n";
        let cancel = CancelToken::new();
        let mut events = |_event: BackendEvent| {};
        let error =
            read_stream(std::io::Cursor::new(data), 16, 1024, &cancel, &mut events).unwrap_err();
        assert_eq!(error, BackendError::Crashed);
    }

    #[test]
    fn final_progress_reaches_max_tokens() {
        let data = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\ndata: [DONE]\n";
        let cancel = CancelToken::new();
        let mut last = 0;
        let mut events = |event: BackendEvent| {
            if event.phase == Phase::Generate {
                last = event.tokens_done;
            }
        };
        read_stream(std::io::Cursor::new(data), 16, 1024, &cancel, &mut events).unwrap();
        assert_eq!(last, 16);
    }

    #[test]
    fn oversized_stream_is_capped() {
        let line = "data: {\"choices\":[{\"delta\":{\"content\":\"xxxxxxxx\"}}]}\n";
        let data = line.repeat(2000);
        let cancel = CancelToken::new();
        let mut events = |_event: BackendEvent| {};
        let error =
            read_stream(std::io::Cursor::new(data), 16, 1024, &cancel, &mut events).unwrap_err();
        assert_eq!(error, BackendError::Crashed);
        let good = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\ndata: [DONE]\n";
        let text = read_stream(
            std::io::Cursor::new(good.as_bytes().to_vec()),
            16,
            1024,
            &cancel,
            &mut events,
        )
        .unwrap();
        assert_eq!(text, "ok");
    }
}
