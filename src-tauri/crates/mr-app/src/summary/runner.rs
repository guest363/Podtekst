use std::sync::Arc;

use serde_json::json;

use super::backend::{BackendEvent, Phase, SummaryBackend};
use super::job::{job_cell, Job};
use super::pipeline;
#[cfg(not(target_os = "android"))]
use super::sidecar;
use super::{activity_finish, backend, emit_event, fake, SummaryJobError};

/// RAII: паника или ранний выход не оставляет вечный `summarizing` и живой
/// счётчик SSE. Снимает только свой job, не чужой.
pub(super) struct JobFinish {
    pub id: String,
}

impl Drop for JobFinish {
    fn drop(&mut self) {
        let owned = {
            let mut cell = job_cell();
            if cell.as_ref().is_some_and(|job| job.id == self.id) {
                *cell = None;
                true
            } else {
                false
            }
        };
        if owned {
            activity_finish();
        }
        #[cfg(not(target_os = "android"))]
        sidecar::arm_idle_monitor();
    }
}

pub(super) fn run_pipeline(
    job: &Arc<Job>,
    spec: &pipeline::JobSpec,
    threads: u32,
) -> Result<pipeline::PipelineOutput, SummaryJobError> {
    let fake = backend::fake_mode();
    let mut backend: Box<dyn SummaryBackend> = if let Some(mode) = fake {
        Box::new(fake::FakeBackend::new(mode))
    } else {
        #[cfg(not(target_os = "android"))]
        {
            let runtime =
                super::runtime_locator::locate().map_err(|_| SummaryJobError::ModelLoad)?;
            Box::new(sidecar::SidecarBackend::new(
                runtime,
                spec.model_sha.clone(),
                threads,
            ))
        }
        #[cfg(target_os = "android")]
        {
            let _ = threads;
            return Err(SummaryJobError::ModelLoad);
        }
    };
    let mut on_event = |event: BackendEvent| {
        let (phase, pct, chunk, chunks) = match event.phase {
            Phase::Load => ("load", 2, None, None),
            Phase::Prepare => ("prepare", 8, None, None),
            Phase::Generate => {
                let total = event.tokens_total.max(1);
                let pct = 10 + (event.tokens_done.min(total) * 80 / total) as u8;
                ("generate", pct.min(90), None, None)
            }
            Phase::Map => {
                let total = event.tokens_total.max(1);
                let pct = 10 + (event.tokens_done.min(total) * 75 / total) as u8;
                ("map", pct.min(85), Some(event.tokens_done + 1), Some(total))
            }
            Phase::Reduce => ("reduce", 88, None, None),
            Phase::Verify => ("verify", 92, None, None),
        };
        stage(job, phase, pct, chunk, chunks);
    };
    let cancel = job.cancel.clone();
    let result = pipeline::run(spec, backend.as_mut(), &cancel, &mut on_event);
    backend.shutdown();
    cancel_sidecar(job);
    if let Err(error) = &result {
        if job.cancel.is_cancelled() && *error != SummaryJobError::OutputCapped {
            return Err(SummaryJobError::Cancelled);
        }
    }
    result
}

/// Отмена обязана погасить именно generation-child, а не ждать idle-таймер:
/// иначе llama-server продолжает считать токены после ухода job. Kill идёт до
/// `JobFinish::drop`, поэтому слот и счётчик освобождаются после join ребёнка.
pub(super) fn cancel_sidecar(job: &Arc<Job>) {
    #[cfg(not(target_os = "android"))]
    if job.cancel.is_cancelled() {
        sidecar::kill_current();
    }
    #[cfg(target_os = "android")]
    let _ = job;
}

fn stage(job: &Arc<Job>, phase: &'static str, pct: u8, chunk: Option<u32>, chunks: Option<u32>) {
    job.touch_progress();
    let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
    if pct <= state.pct && state.phase == phase {
        return;
    }
    state.phase = phase;
    state.pct = state.pct.max(pct);
    state.chunk = chunk;
    state.chunks = chunks;
    let event = json!({
        "type": "summary-stage",
        "job_id": job.id,
        "file": job.file,
        "phase": phase,
        "pct": state.pct,
        "chunk": chunk,
        "chunks": chunks,
    });
    drop(state);
    emit_event(event);
}
