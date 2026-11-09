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
            let cpu = super::runtime_locator::locate().map_err(|_| SummaryJobError::ModelLoad)?;
            let gpu = if super::config::load().gpu == super::config::GpuMode::Auto {
                super::gpu::installed_runtime()
            } else {
                None
            };
            let (runtime, gpu_layers, gpu_device, gpu_backend, fallback) = match gpu {
                Some(gpu) => (
                    gpu.runtime,
                    99,
                    gpu.device_index,
                    gpu.backend,
                    Some(cpu.clone()),
                ),
                None => (cpu.clone(), 0, None, None, None),
            };
            Box::new(sidecar::SidecarBackend::new(
                &runtime,
                spec.model_sha.clone(),
                threads,
                gpu_layers,
                gpu_device,
                gpu_backend,
                fallback,
            ))
        }
        #[cfg(target_os = "android")]
        {
            let _ = threads;
            return Err(SummaryJobError::ModelLoad);
        }
    };
    let mut on_event = |event: BackendEvent| stage(job, event);
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

fn stage_progress(
    current_phase: &'static str,
    current_pct: u8,
    current_chunk: Option<u32>,
    current_chunks: Option<u32>,
    event: BackendEvent,
) -> (&'static str, u8, Option<u32>, Option<u32>) {
    match event.phase {
        Phase::Load => ("load", 2, None, None),
        Phase::Prepare => ("prepare", 8, None, None),
        Phase::Generate => {
            let total = event.tokens_total.max(1);
            let done = event.tokens_done.min(total);
            let sub = (done * 80 / total) as u8;
            // Второй проход (reduce/repair) идёт уже после 88/92%; не
            // откатываем шкалу назад, а двигаем её в хвосте фазы.
            match current_phase {
                "map" => ("map", current_pct, current_chunk, current_chunks),
                "reduce" => ("reduce", 88 + sub / 20, None, None),
                "verify" => ("verify", 92 + sub / 20, None, None),
                _ => ("generate", 10 + sub, None, None),
            }
        }
        Phase::Map => {
            let total = event.tokens_total.max(1);
            let pct = 10 + (event.tokens_done.min(total) * 75 / total) as u8;
            ("map", pct.min(85), Some(event.tokens_done + 1), Some(total))
        }
        Phase::Reduce => ("reduce", 88, None, None),
        Phase::Verify => ("verify", 92, None, None),
    }
}

fn stage(job: &Arc<Job>, event: BackendEvent) {
    job.touch_progress();
    let mut state = job.state.lock().unwrap_or_else(|e| e.into_inner());
    let (phase, pct, chunk, chunks) =
        stage_progress(state.phase, state.pct, state.chunk, state.chunks, event);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event(phase: Phase, done: u32, total: u32) -> BackendEvent {
        BackendEvent {
            phase,
            tokens_done: done,
            tokens_total: total,
        }
    }

    #[test]
    fn generate_after_reduce_moves_tail() {
        let (phase, pct, _, _) =
            stage_progress("reduce", 88, None, None, event(Phase::Generate, 40, 80));
        assert_eq!((phase, pct), ("reduce", 90));
    }

    #[test]
    fn generate_after_verify_moves_tail() {
        let (phase, pct, _, _) =
            stage_progress("verify", 92, None, None, event(Phase::Generate, 40, 80));
        assert_eq!((phase, pct), ("verify", 94));
    }

    #[test]
    fn generate_during_map_keeps_map_and_chunk() {
        let (phase, pct, chunk, chunks) =
            stage_progress("map", 55, Some(2), Some(4), event(Phase::Generate, 40, 80));
        assert_eq!((phase, pct, chunk, chunks), ("map", 55, Some(2), Some(4)));
    }

    #[test]
    fn first_generate_uses_main_scale() {
        let (phase, pct, _, _) =
            stage_progress("prepare", 8, None, None, event(Phase::Generate, 40, 80));
        assert_eq!((phase, pct), ("generate", 50));
    }
}
