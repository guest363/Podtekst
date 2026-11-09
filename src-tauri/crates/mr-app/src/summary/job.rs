use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mr_summarize::Detail;
use serde_json::{json, Value};

use super::backend::CancelToken;
use super::catalog::{model_path, model_ready, verified_model_sha};
use super::config::{self, MODEL_SHA256};
use super::memory;
use super::pipeline;
use super::runner::{run_pipeline, JobFinish};
#[cfg(not(target_os = "android"))]
use super::sidecar;
use super::store;
use super::{
    activity_finish, activity_start, emit_event, id, runtime_ready, supported, ApiError,
    SummaryJobError,
};

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
    last_progress: Mutex<Instant>,
}

impl Job {
    /// Watchdog смотрит на прогресс, а не на общий wall-clock: длинная
    /// CPU-встреча может законно идти 10–20 минут.
    pub(super) fn touch_progress(&self) {
        *self.last_progress.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn progress_elapsed(&self) -> Duration {
        self.last_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
    }
}

pub(super) static JOB: Mutex<Option<Arc<Job>>> = Mutex::new(None);

pub(super) fn job_cell() -> std::sync::MutexGuard<'static, Option<Arc<Job>>> {
    JOB.lock().unwrap_or_else(|e| e.into_inner())
}

/// Слот резервируется под тем же мьютексом, что и публикуется job: две
/// одновременные заявки не могут обе пройти preflight. Ошибка на любом
/// пути освобождает слот и счётчик активности через Drop.
struct Reservation {
    job: Arc<Job>,
    armed: bool,
}

impl Reservation {
    fn job(&self) -> &Arc<Job> {
        &self.job
    }

    fn disarm(mut self) -> Arc<Job> {
        self.armed = false;
        self.job.clone()
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        {
            let mut cell = job_cell();
            if !cell.as_ref().is_some_and(|job| job.id == self.job.id) {
                return;
            }
            *cell = None;
        }
        activity_finish();
    }
}

fn reserve(file: String) -> Result<Reservation, ApiError> {
    let mut cell = job_cell();
    if cell.is_some() {
        return Err(ApiError::new(
            409,
            "summary_busy",
            "Итоги уже собираются для другой записи. Повторите позже.",
        ));
    }
    let job = Arc::new(Job {
        id: id::new_job_id(),
        file,
        started: Instant::now(),
        cancel: CancelToken::new(),
        timed_out: AtomicBool::new(false),
        state: Mutex::new(JobState {
            phase: "load",
            pct: 0,
            chunk: None,
            chunks: None,
        }),
        last_progress: Mutex::new(Instant::now()),
    });
    *cell = Some(job.clone());
    drop(cell);
    activity_start();
    Ok(Reservation { job, armed: true })
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
    let reservation = reserve(paths.relative.clone())?;
    let source = super::cache::read(&paths).map_err(ApiError::store)?;
    if source.rev != input.source_rev {
        return Err(ApiError::new(
            409,
            "stale_source",
            "Расшифровка изменилась. Итоги не сохранены. Повторите.",
        ));
    }
    let loaded = store::load(&paths, &source.rev);
    // Новее-major артефакт заменяется только при явных force и allow_downgrade:
    // одного allow_downgrade недостаточно.
    if loaded.state == store::State::Unsupported && !(input.allow_downgrade && input.force) {
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
    let model_sha = if fake {
        MODEL_SHA256.to_string()
    } else {
        if super::headless() && !runtime_ready() {
            return Err(ApiError::new(
                503,
                "runtime_missing",
                "Итоги недоступны: локальный движок не найден.",
            ));
        }
        let Some(path) = model_path() else {
            return Err(ApiError::new(
                503,
                "model_missing",
                "Модель итогов не скачана.",
            ));
        };
        let Some(sha) = verified_model_sha(&path) else {
            return Err(ApiError::new(
                503,
                "model_missing",
                "Модель итогов не читается. Скачайте её заново.",
            ));
        };
        if sha != MODEL_SHA256 {
            return Err(ApiError::new(
                422,
                "model_corrupt",
                "Модель итогов повреждена. Скачайте её заново.",
            ));
        }
        if !runtime_ready() {
            return Err(ApiError::new(
                503,
                "runtime_missing",
                "Итоги недоступны: локальный движок не найден.",
            ));
        }
        sha
    };
    #[cfg(not(target_os = "android"))]
    let sidecar_loaded = sidecar::loaded_model_sha().as_deref() == Some(model_sha.as_str());
    #[cfg(target_os = "android")]
    let sidecar_loaded = false;
    // Fake-режим существует только в debug и не грузит модель: проверки
    // памяти и диска относятся к настоящему sidecar, поэтому здесь они
    // только ломали бы контрактные тесты на слабых CI-раннерах.
    let guard = if fake {
        memory::GuardInfo {
            threads: memory::threads(memory::logical_cores()),
        }
    } else {
        memory::precheck(sidecar_loaded)
            .map_err(|error| ApiError::new(422, "low_memory", error.user_message()))?
    };
    if !fake && memory::free_disk_mb(&paths.parent).is_some_and(|free| free < 512) {
        return Err(ApiError::new(
            422,
            "disk_low",
            "Не хватает места на диске. Освободите место и повторите.",
        ));
    }
    let job_id = reservation.job().id.clone();
    let job = reservation.job().clone();
    let spec = pipeline::JobSpec {
        paths,
        preset_id: input.preset_id.clone(),
        detail,
        confirmed_sensitive: input.confirm_sensitive,
        allow_downgrade: input.allow_downgrade,
        force: input.force,
        model_sha,
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
        crate::note!("summary: spawn failed: {error}");
        return Err(ApiError::new(
            500,
            "internal",
            "Внутренняя ошибка итогов. Повторите и лог.",
        ));
    }
    arm_timeout(reservation.disarm());
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

/// Watchdog: 15 минут без единого события прогресса или 4 часа абсолютного
/// времени. Общий wall-clock не годится — 20-минутная встреча на CPU идёт
/// 11 минут и это нормально.
fn arm_timeout(job: Arc<Job>) {
    const STALL: Duration = Duration::from_secs(15 * 60);
    const ABSOLUTE: Duration = Duration::from_secs(4 * 60 * 60);
    std::thread::spawn(move || loop {
        if !is_current(&job.id) {
            return;
        }
        if job.cancel.is_cancelled() {
            return;
        }
        if job.progress_elapsed() < STALL && job.started.elapsed() < ABSOLUTE {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        job.timed_out.store(true, Ordering::SeqCst);
        job.cancel.cancel();
        std::thread::sleep(Duration::from_secs(2));
        if is_current(&job.id) {
            #[cfg(not(target_os = "android"))]
            sidecar::kill_current();
        }
        return;
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
                // в 2 с, поэтому через 1 с добиваем sidecar.
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
        if !is_current(&job.id) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !is_current(&job.id) {
        return;
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

#[cfg(all(test, unix, not(target_os = "android")))]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn alive(pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            std::path::Path::new(&format!("/proc/{pid}")).exists()
        }
        #[cfg(not(target_os = "linux"))]
        {
            // SAFETY: signal 0 только проверяет существование PID.
            unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
        }
    }

    #[test]
    fn cancel_kills_generation_child_and_releases_slot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *job_cell() = None;
        let reservation = reserve("cancel.wav".to_string()).expect("слот занят");
        let job_id = reservation.job().id.clone();
        let job = reservation.disarm();
        let pid = sidecar::install_test_child("sleep", &["30"]);
        let worker_job = job.clone();
        let worker = std::thread::spawn(move || {
            while !worker_job.cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(10));
            }
            super::super::runner::cancel_sidecar(&worker_job);
            drop(JobFinish {
                id: worker_job.id.clone(),
            });
        });
        assert!(cancel(&job_id));
        let started = Instant::now();
        while alive(pid) && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(pid), "generation-child пережил отмену");
        worker.join().unwrap();
        assert!(!is_active(), "слот job не освобождён");
        #[cfg(feature = "onnx")]
        assert!(
            !crate::transcribe::events::is_summary_active(),
            "счётчик активности не вернулся в ноль"
        );
        assert!(
            sidecar::test_state_pid().is_none(),
            "sidecar state остался после kill"
        );
    }

    #[test]
    fn progress_touch_resets_watchdog() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *job_cell() = None;
        let reservation = reserve("watchdog.wav".to_string()).expect("слот занят");
        let job = reservation.job().clone();
        std::thread::sleep(Duration::from_millis(20));
        assert!(job.progress_elapsed() >= Duration::from_millis(20));
        job.touch_progress();
        assert!(job.progress_elapsed() < Duration::from_millis(20));
        drop(reservation);
    }

    #[test]
    fn double_reserve_yields_one_slot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *job_cell() = None;
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|i| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    reserve(format!("race-{i}.wav"))
                })
            })
            .collect();
        let winners: Vec<Reservation> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap().ok())
            .collect();
        assert_eq!(winners.len(), 1, "две заявки заняли слот");
        assert!(is_active());
        drop(winners);
        assert!(!is_active(), "drop резерва не освободил слот");
        #[cfg(feature = "onnx")]
        assert!(
            !crate::transcribe::events::is_summary_active(),
            "счётчик после резерва не вернулся в ноль"
        );
    }
}
