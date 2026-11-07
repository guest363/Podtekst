use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mr_summarize::Detail;
use serde_json::{json, Value};

use super::backend::CancelToken;
use super::catalog::{model_path, model_ready};
use super::config::{self, MODEL_SHA256};
use super::memory;
use super::pipeline;
use super::runner::{run_pipeline, JobFinish};
#[cfg(not(target_os = "android"))]
use super::sidecar;
use super::store;
use super::{activity_start, emit_event, id, runtime_ready, supported, ApiError, SummaryJobError};

#[derive(Debug, Clone, Copy)]
pub(super) struct JobState {
    pub phase: &'static str,
    pub pct: u8,
    pub chunk: Option<u32>,
    pub chunks: Option<u32>,
}

pub(super) struct Job {
    pub id: String,
    pub file: String,
    pub started: Instant,
    pub cancel: CancelToken,
    pub timed_out: AtomicBool,
    pub state: Mutex<JobState>,
}

pub(super) static JOB: Mutex<Option<Arc<Job>>> = Mutex::new(None);

pub(super) fn job_cell() -> std::sync::MutexGuard<'static, Option<Arc<Job>>> {
    JOB.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn is_current(job_id: &str) -> bool {
    job_cell()
        .as_ref()
        .map(|job| job.id == job_id)
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct GenerateInput {
    pub file: String,
    pub preset_id: String,
    pub detail: String,
    pub source_rev: String,
    pub force: bool,
    pub confirm_sensitive: bool,
    pub allow_downgrade: bool,
}

#[derive(Debug, Clone)]
pub enum StartOutcome {
    Cached { summary_id: String },
    Started { job_id: String },
}

pub fn is_active() -> bool {
    job_cell().is_some()
}

pub(super) fn active_job() -> Option<Arc<Job>> {
    job_cell().clone()
}

pub fn active_json() -> Option<Value> {
    let job = active_job()?;
    let state = job.state.lock().unwrap_or_else(|e| e.into_inner());
    Some(json!({
        "job_id": job.id,
        "file": job.file,
        "phase": state.phase,
        "pct": state.pct,
        "chunk": state.chunk,
        "chunks": state.chunks,
        "elapsed_sec": job.started.elapsed().as_secs(),
    }))
}

pub fn status_fields() -> Value {
    let job = active_job();
    let (phase, pct, file, id) = match &job {
        Some(job) => {
            let state = job.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.phase.to_string(),
                state.pct,
                job.file.clone(),
                job.id.clone(),
            )
        }
        None => ("".to_string(), 0, String::new(), String::new()),
    };
    json!({
        "summary_supported": supported(),
        "summary_runtime": if runtime_ready() { "sidecar" } else { "missing" },
        "summary_model_ready": model_ready(),
        "summary_classifier": config::load().classifier.as_str(),
        "summarizing": job.is_some(),
        "summarizing_file": file,
        "summary_job_id": id,
        "summary_phase": phase,
        "summary_pct": pct,
        "summary_block_reason": super::block_reason(false),
    })
}

pub fn generate(input: &GenerateInput, recording: bool) -> Result<StartOutcome, ApiError> {
    super::ensure_enabled()?;
    if recording {
        return Err(ApiError::new(
            409,
            "recording_active",
            "Идёт запись. Итоги можно собрать после её окончания.",
        ));
    }
    if super::transcription_pending() {
        return Err(ApiError::new(
            409,
            "transcription_running",
            "Идёт расшифровка. Итоги можно собрать после её окончания.",
        ));
    }
    if is_active() {
        return Err(ApiError::new(
            409,
            "summary_busy",
            "Итоги уже собираются для другой записи. Повторите позже.",
        ));
    }
    let preset = mr_summarize::preset::get(&input.preset_id)
        .ok_or_else(|| ApiError::new(400, "invalid_preset", "Неизвестный шаблон."))?;
    let detail = Detail::parse(&input.detail)
        .map_err(|_| ApiError::new(400, "invalid_detail", "Неизвестная подробность."))?;
    if preset.sensitive && !input.confirm_sensitive {
        return Err(ApiError::new(
            400,
            "confirmation_required",
            "Для этого шаблона нужно подтверждение.",
        ));
    }
    if input.source_rev.trim().is_empty() {
        return Err(ApiError::new(
            400,
            "source_rev_required",
            "Не передана версия расшифровки.",
        ));
    }
    let paths = store::resolve(&input.file).map_err(ApiError::store)?;
    let source = super::cache::read(&paths).map_err(ApiError::store)?;
    if source.rev != input.source_rev {
        return Err(ApiError::new(
            409,
            "stale_source",
            "Расшифровка изменилась. Итоги не сохранены. Повторите.",
        ));
    }
    let loaded = store::load(&paths, &source.rev);
    if loaded.state == store::State::Unsupported && !input.allow_downgrade {
        return Err(ApiError::new(
            422,
            "schema_unsupported",
            "Эта версия не поддерживает формат итогов.",
        ));
    }
    if let Some(artifact) = loaded.artifact.as_ref() {
        if loaded.state == store::State::Fresh && !input.force && cache_matches(artifact, input) {
            return Ok(StartOutcome::Cached {
                summary_id: artifact.id.clone(),
            });
        }
    }
    let fake = super::backend::fake_mode().is_some();
    if !fake {
        if model_path().is_none() {
            return Err(ApiError::new(
                503,
                "model_missing",
                "Модель итогов не скачана.",
            ));
        }
        if !runtime_ready() {
            return Err(ApiError::new(
                503,
                "runtime_missing",
                "Итоги недоступны: локальный движок не найден.",
            ));
        }
    }
    #[cfg(not(target_os = "android"))]
    let sidecar_loaded = sidecar::loaded_model_sha().as_deref() == Some(MODEL_SHA256);
    #[cfg(target_os = "android")]
    let sidecar_loaded = false;
    let guard = memory::precheck(sidecar_loaded)
        .map_err(|error| ApiError::new(422, "low_memory", error.user_message()))?;
    if memory::free_disk_mb(&paths.parent).is_some_and(|free| free < 512) {
        return Err(ApiError::new(
            422,
            "disk_low",
            "Не хватает места на диске. Освободите место и повторите.",
        ));
    }
    let job_id = id::new_job_id();
    let job = Arc::new(Job {
        id: job_id.clone(),
        file: paths.relative.clone(),
        started: Instant::now(),
        cancel: CancelToken::new(),
        timed_out: AtomicBool::new(false),
        state: Mutex::new(JobState {
            phase: "load",
            pct: 0,
            chunk: None,
            chunks: None,
        }),
    });
    *job_cell() = Some(job.clone());
    activity_start();
    let spec = pipeline::JobSpec {
        paths,
        preset_id: input.preset_id.clone(),
        detail,
        confirmed_sensitive: input.confirm_sensitive,
        allow_downgrade: input.allow_downgrade,
    };
    let worker_job = job.clone();
    let spawn = std::thread::Builder::new()
        .name("summary-job".to_string())
        .spawn(move || {
            let _finish = JobFinish {
                id: worker_job.id.clone(),
            };
            let result = run_pipeline(&worker_job, &spec, guard.threads);
            match result {
                Ok(output) => {
                    if !output.md_written {
                        crate::note!("summary: .summary.md не записался, JSON валиден");
                    }
                    emit_event(json!({
                        "type": "summary-done",
                        "job_id": worker_job.id,
                        "file": worker_job.file,
                        "ok": true,
                        "duration_ms": worker_job.started.elapsed().as_millis() as u64,
                        "summary_id": output.artifact.id,
                    }));
                }
                Err(error) => {
                    let error = if worker_job.timed_out.load(Ordering::SeqCst) {
                        SummaryJobError::Timeout
                    } else {
                        error
                    };
                    let phase = worker_job
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .phase;
                    emit_event(json!({
                        "type": "summary-error",
                        "job_id": worker_job.id,
                        "file": worker_job.file,
                        "code": error.code(),
                        "phase": phase,
                        "retryable": error.retryable(),
                    }));
                }
            }
        });
    if let Err(error) = spawn {
        *job_cell() = None;
        super::activity_finish();
        crate::note!("summary: spawn failed: {error}");
        return Err(ApiError::new(
            500,
            "internal",
            "Внутренняя ошибка итогов. Повторите и лог.",
        ));
    }
    arm_timeout(job, source.transcript.total_chars());
    Ok(StartOutcome::Started { job_id })
}

fn cache_matches(artifact: &mr_summarize::SummaryArtifact, input: &GenerateInput) -> bool {
    artifact.preset.id == input.preset_id
        && artifact.preset.version == mr_summarize::preset::PRESET_VERSION
        && artifact.detail.as_str() == input.detail
        && artifact.generator.model_sha256 == MODEL_SHA256
        && artifact.generator.prompt_version
            == mr_summarize::preset::get(&input.preset_id)
                .map(|p| p.prompt_version)
                .unwrap_or_default()
}

fn arm_timeout(job: Arc<Job>, chars: usize) {
    let secs = (120 + chars / 20).clamp(300, 900) as u64;
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if !is_current(&job.id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !is_current(&job.id) {
            return;
        }
        job.timed_out.store(true, Ordering::SeqCst);
        job.cancel.cancel();
        std::thread::sleep(Duration::from_secs(2));
        if is_current(&job.id) {
            #[cfg(not(target_os = "android"))]
            sidecar::kill_current();
        }
    });
}

pub fn cancel(job_id: &str) -> bool {
    let job = active_job();
    match job {
        Some(job) if job.id == job_id => {
            job.cancel.cancel();
            let id = job.id.clone();
            std::thread::spawn(move || {
                // Prefill может не отдавать токены: отмена обязана уложиться
                // в 2 с, поэтому через 1 с добиваем sidecar (G2).
                std::thread::sleep(Duration::from_secs(1));
                if is_current(&id) {
                    #[cfg(not(target_os = "android"))]
                    sidecar::kill_current();
                }
            });
            true
        }
        _ => false,
    }
}

fn cancel_active() {
    let Some(job) = active_job() else {
        return;
    };
    job.cancel.cancel();
    for _ in 0..20 {
        if !is_active() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    #[cfg(not(target_os = "android"))]
    sidecar::kill_current();
}

pub fn cancel_for_transcription() {
    cancel_active();
}

pub fn cancel_for_recording() {
    cancel_active();
}
