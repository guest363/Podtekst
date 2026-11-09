use std::collections::HashMap;
use std::time::Instant;

use mr_ml::onnx::vad::{SileroVad, VadRunStats, VAD_WINDOW};

use super::super::channel_mix;
use super::super::diag::{
    log_vad_segments, log_vad_summary, log_vad_windows, stage_begin, stage_end,
    VAD_MIN_SILENCE_SEC, VAD_MIN_SPEECH_SEC, VAD_THRESHOLD,
};
use super::super::diarize;
use super::{MAX_SEGMENT_SEC, SEGMENT_GAP_SEC};

type VadOutput = (HashMap<usize, Vec<f32>>, HashMap<usize, Vec<(f64, f64)>>);

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
    if is_stereo {
        if let (Some(mic), Some(system)) = (channel_samples.get(&0), channel_samples.get(&1)) {
            if let Some(probs) = channel_vad.get_mut(&0) {
                let stats = channel_mix::suppress_mic_bleed_with_stats(mic, system, probs);
                log::info!(
                    target: "audio",
                    ch = 0,
                    delay = stats.delay,
                    prom = stats.prominence,
                    suppressed = stats.suppressed,
                    total = stats.frames,
                    probe_sec = stats.probe_sec;
                    "ev=audio.bleed"
                );
                if stats.suppressed > 0 {
                    on_event(
                        14,
                        &format!("Микрофон: подавлено эхо в {} кадрах", stats.suppressed),
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
        let segs = log_vad_summary(*ch as i32, probs, stats);
        log_vad_windows(*ch as i32, samples, probs);
        log_vad_segments(*ch as i32, probs, &segs);
        let grouped = diarize::merge_quiet_short(
            samples,
            diarize::group_segments(&segs, probs, MAX_SEGMENT_SEC, SEGMENT_GAP_SEC, *ch as i32),
            *ch as i32,
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
    Ok((channel_vad, channel_grouped))
}
