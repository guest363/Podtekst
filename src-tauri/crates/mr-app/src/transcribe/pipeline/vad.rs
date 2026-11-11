use std::collections::HashMap;
use std::time::Instant;

use mr_ml::onnx::vad::{SileroVad, VadRunStats, VAD_WINDOW};

use super::super::channel_mix;
use super::super::diag::{
    log_vad_segments, log_vad_summary, log_vad_windows, stage_begin, stage_end,
    VAD_MIN_SILENCE_SEC, VAD_MIN_SPEECH_SEC, VAD_THRESHOLD,
};
use super::super::diarize;
use super::super::echo::{self, EchoEvidence};
use super::{MAX_SEGMENT_SEC, SEGMENT_GAP_SEC};

/// Сегмент выбрасывается, только если эхо в нём почти без локальной речи:
/// двойная речь и «неясно» остаются, иначе теряется тихая локальная фраза.
const ECHO_SEGMENT_ECHO_ONLY: f32 = 0.80;
const ECHO_SEGMENT_DOUBLE_TALK: f32 = 0.05;
const ECHO_SEGMENT_LOCAL_ONLY: f32 = 0.15;
const MIN_LOCAL_KEEP_SEC: f64 = 0.3;

pub(super) struct VadOutput {
    pub(super) channel_vad: HashMap<usize, Vec<f32>>,
    pub(super) channel_grouped: HashMap<usize, Vec<(f64, f64)>>,
    pub(super) echo: Option<EchoEvidence>,
}

fn filter_echo_segments(
    segments: Vec<(f32, f32)>,
    evidence: &EchoEvidence,
    ch: i32,
) -> Vec<(f32, f32)> {
    let total = segments.len();
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(total);
    let mut dropped = 0usize;
    for (start, end) in segments {
        let profile = echo::segment_profile(evidence, start as f64, end as f64);
        if profile.echo_only >= ECHO_SEGMENT_ECHO_ONLY
            && profile.double_talk <= ECHO_SEGMENT_DOUBLE_TALK
            && profile.local_only <= ECHO_SEGMENT_LOCAL_ONLY
        {
            log::debug!(
                target: "vad",
                ch = ch,
                at_sec = start,
                dur_sec = end - start,
                echo_only = profile.echo_only,
                residual = echo::mean_residual(evidence, start as f64, end as f64);
                "ev=vad.echo_drop"
            );
            dropped += 1;
            continue;
        }
        match echo::trim_echo_edges(evidence, start as f64, end as f64, MIN_LOCAL_KEEP_SEC) {
            Some((trimmed_start, trimmed_end)) => {
                out.push((trimmed_start as f32, trimmed_end as f32));
            }
            None => dropped += 1,
        }
    }
    if dropped > 0 {
        log::debug!(
            target: "vad",
            ch = ch,
            dropped = dropped,
            total = total;
            "ev=vad.echo_filter"
        );
    }
    out
}

pub(super) fn run(
    channel_samples: &HashMap<usize, Vec<f32>>,
    chan_list: &[(usize, String)],
    is_stereo: bool,
    vad: &mut SileroVad,
    on_event: &dyn Fn(u8, &str),
) -> Result<VadOutput, String> {
    let mut channel_grouped: HashMap<usize, Vec<(f64, f64)>> = HashMap::new();
    let mut channel_vad: HashMap<usize, Vec<f32>> = HashMap::new();
    let mut channel_vad_stats: HashMap<usize, VadRunStats> = HashMap::new();
    let channel_count = chan_list.len().max(1) as f64;
    let vad_stage = Instant::now();
    stage_begin("vad");
    for (ch_i, (ch, label)) in chan_list.iter().enumerate() {
        let samples = &channel_samples[ch];
        let base = 6.0 + 8.0 * ch_i as f64 / channel_count;
        let span = 8.0 / channel_count;
        let windows = samples.len().div_ceil(VAD_WINDOW);
        log::debug!(
            target: "vad",
            ch = *ch as i64,
            samples = samples.len(),
            windows = windows,
            threshold = VAD_THRESHOLD,
            min_speech_sec = VAD_MIN_SPEECH_SEC,
            min_silence_sec = VAD_MIN_SILENCE_SEC;
            "ev=vad.start"
        );
        let (probs, stats) = vad
            .process_all_with_stats(samples, |done, total| {
                let pct = (base + span * done as f64 / total.max(1) as f64) as u8;
                on_event(
                    pct.clamp(6, 14),
                    &format!("Поиск речи ({label}): {done} из {total} окон"),
                );
            })
            .map_err(|e| format!("VAD: {e}"))?;
        channel_vad_stats.insert(*ch, stats);
        channel_vad.insert(*ch, probs);
    }
    let mut evidence: Option<EchoEvidence> = None;
    if is_stereo {
        if let (Some(mic), Some(system)) = (channel_samples.get(&0), channel_samples.get(&1)) {
            let mic_probs = channel_vad.get(&0).map(Vec::as_slice);
            evidence = channel_mix::classify_echo(mic, system, mic_probs);
            if let Some(echo) = &evidence {
                let suppressed = channel_vad
                    .get_mut(&0)
                    .map(|probs| channel_mix::apply_vad_gate(probs, echo))
                    .unwrap_or(0);
                log::info!(
                    target: "audio",
                    ch = 0,
                    delay = echo.delay_samples,
                    delay_sec = echo.delay_sec,
                    prom = echo.prominence,
                    stability = echo.stability,
                    gain = echo.gain,
                    noise_floor = echo.noise_floor,
                    suppressed = suppressed,
                    total = echo.frames.len(),
                    probe_sec = echo::PROBE_SEC as f64;
                    "ev=audio.bleed"
                );
                if suppressed > 0 {
                    on_event(
                        14,
                        &format!("Микрофон: подавлено эхо в {suppressed} кадрах"),
                    );
                }
            }
        }
    }
    for (ch_i, (ch, label)) in chan_list.iter().enumerate() {
        let samples = &channel_samples[ch];
        let base = 6.0 + 8.0 * ch_i as f64 / channel_count;
        let span = 8.0 / channel_count;
        let probs = &channel_vad[ch];
        let stats = channel_vad_stats.get(ch).copied().unwrap_or_default();
        let mut segs = log_vad_summary(*ch as i32, probs, stats);
        if *ch == 0 {
            if let Some(echo) = &evidence {
                segs = filter_echo_segments(segs, echo, *ch as i32);
            }
        }
        log_vad_windows(*ch as i32, samples, probs);
        log_vad_segments(*ch as i32, probs, &segs);
        let grouped = diarize::merge_quiet_short_with_evidence(
            samples,
            diarize::group_segments(&segs, probs, MAX_SEGMENT_SEC, SEGMENT_GAP_SEC, *ch as i32),
            *ch as i32,
            if *ch == 0 { evidence.as_ref() } else { None },
            if *ch == 0 {
                channel_samples.get(&1).map(Vec::as_slice)
            } else {
                None
            },
        );
        if cfg!(debug_assertions) && crate::logging::dump_enabled() {
            let channel = format!("ch{ch}");
            crate::logging::write_artifact_f32(&format!("vad/{channel}.probs.f32"), probs);
            crate::logging::write_artifact_json(
                &format!("vad/{channel}.segments.json"),
                &serde_json::json!({
                    "raw": segs,
                    "grouped": grouped,
                }),
            );
        }
        on_event(
            (base + span) as u8,
            &format!("{label}: найдено {} фрагментов", grouped.len()),
        );
        channel_grouped.insert(
            *ch,
            grouped
                .into_iter()
                .map(|(a, b)| (a as f64, b as f64))
                .collect(),
        );
    }
    stage_end("vad", vad_stage);
    Ok(VadOutput {
        channel_vad,
        channel_grouped,
        echo: evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::echo::FrameClass;

    const WIN: f64 = VAD_WINDOW as f64 / 16_000.0;

    fn evidence(spans: &[(f64, f64, FrameClass)], total_sec: f64) -> EchoEvidence {
        let n = (total_sec / WIN).ceil() as usize;
        let frames = (0..n)
            .map(|i| {
                let t = i as f64 * WIN;
                spans
                    .iter()
                    .find(|(a, b, _)| t >= *a && t < *b)
                    .map(|(_, _, class)| *class)
                    .unwrap_or(FrameClass::Uncertain)
            })
            .collect::<Vec<_>>();
        EchoEvidence {
            delay_samples: 1000,
            delay_sec: 1000.0 / 16_000.0,
            prominence: 12.0,
            stability: 1.0,
            gain: 0.1,
            noise_floor: 1e-4,
            residual: vec![0.0; n],
            frames,
        }
    }

    #[test]
    fn echo_dominant_segment_is_dropped() {
        let ev = evidence(
            &[
                (0.0, 3.0, FrameClass::EchoOnly),
                (3.0, 6.0, FrameClass::LocalOnly),
            ],
            6.0,
        );
        let out = filter_echo_segments(vec![(0.0, 3.0), (3.0, 6.0)], &ev, 0);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(
            (out[0].0 - 3.0).abs() < 0.05 && (out[0].1 - 6.0).abs() < 0.05,
            "{out:?}"
        );
    }

    #[test]
    fn segment_with_local_speech_is_kept() {
        let ev = evidence(
            &[
                (0.0, 2.0, FrameClass::DoubleTalk),
                (2.0, 5.0, FrameClass::Uncertain),
            ],
            5.0,
        );
        let out = filter_echo_segments(vec![(0.0, 5.0)], &ev, 0);
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn echo_edges_are_trimmed_from_mixed_segment() {
        let ev = evidence(
            &[
                (0.0, 2.0, FrameClass::EchoOnly),
                (2.0, 5.0, FrameClass::LocalOnly),
            ],
            5.0,
        );
        let out = filter_echo_segments(vec![(0.0, 5.0)], &ev, 0);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].0 >= 2.0, "{out:?}");
        assert!((out[0].1 - 5.0).abs() < 0.05, "{out:?}");
    }

    #[test]
    fn segment_without_evidence_is_untouched() {
        let segs = vec![(0.0, 1.0), (2.0, 4.0)];
        let ev = evidence(&[], 4.0);
        assert_eq!(filter_echo_segments(segs.clone(), &ev, 0), segs);
    }
}
