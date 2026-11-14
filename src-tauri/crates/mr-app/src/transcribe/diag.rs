use std::collections::HashMap;
use std::time::Instant;

use mr_ml::onnx::vad::{vad_segments_from_probs, VadRunStats, VAD_WINDOW};

use super::asr_engine::models_root;

pub(super) const VAD_THRESHOLD: f32 = 0.30;
pub(super) const VAD_NEG_THRESHOLD: f32 = 0.15;
pub(super) const VAD_MIN_SPEECH_SEC: f32 = 0.12;
pub(super) const VAD_MIN_SILENCE_SEC: f32 = 0.30;
pub(super) const VAD_SPEECH_PAD_SEC: f32 = 0.25;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AudioStats {
    pub(super) rms: f64,
    pub(super) peak: f32,
    pub(super) clip_pct: f64,
    pub(super) silence_pct: f64,
    pub(super) nan: usize,
    pub(super) inf: usize,
}

pub(super) fn stage_begin(name: &'static str) {
    if !cfg!(debug_assertions) {
        return;
    }
    crate::logging::enter_stage(name);
    log::debug!(target: "run", "ev=stage.start");
}

pub(super) fn stage_end(_name: &'static str, started: Instant) {
    if !cfg!(debug_assertions) {
        return;
    }
    log::debug!(
        target: "run",
        elapsed_ms = started.elapsed().as_millis() as u64;
        "ev=stage.end"
    );
}

pub(super) fn audio_stats(samples: &[f32]) -> AudioStats {
    if samples.is_empty() {
        return AudioStats::default();
    }
    let mut sum_sq = 0.0f64;
    let mut peak = 0.0f32;
    let mut clipped = 0usize;
    let mut silent = 0usize;
    let mut nan = 0usize;
    let mut inf = 0usize;
    for &sample in samples {
        if sample.is_nan() {
            nan += 1;
            continue;
        }
        if sample.is_infinite() {
            inf += 1;
            continue;
        }
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_sq += f64::from(sample) * f64::from(sample);
        if abs >= 0.999 {
            clipped += 1;
        }
        if abs < 1e-4 {
            silent += 1;
        }
    }
    let finite = samples.len().saturating_sub(nan + inf).max(1);
    AudioStats {
        rms: (sum_sq / finite as f64).sqrt(),
        peak,
        clip_pct: clipped as f64 * 100.0 / finite as f64,
        silence_pct: silent as f64 * 100.0 / finite as f64,
        nan,
        inf,
    }
}

pub(super) fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f32 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn finite_probs(probs: &[f32]) -> Vec<f32> {
    let mut values: Vec<f32> = probs.iter().copied().filter(|p| p.is_finite()).collect();
    values.sort_by(f32::total_cmp);
    values
}

pub(super) fn speech_segments(probs: &[f32]) -> Vec<(f32, f32)> {
    vad_segments_from_probs(
        probs,
        VAD_THRESHOLD,
        VAD_NEG_THRESHOLD,
        VAD_MIN_SPEECH_SEC,
        VAD_MIN_SILENCE_SEC,
        VAD_SPEECH_PAD_SEC,
    )
}

pub(super) fn log_vad_summary(ch: i32, probs: &[f32], stats: VadRunStats) -> Vec<(f32, f32)> {
    let segments = speech_segments(probs);
    if !cfg!(debug_assertions) {
        return segments;
    }
    let speech_win = probs.iter().filter(|&&p| p >= VAD_THRESHOLD).count();
    let speech_sec: f32 = segments.iter().map(|(a, b)| b - a).sum::<f32>().max(0.0);
    let max_seg_sec = segments.iter().map(|(a, b)| b - a).fold(0.0f32, f32::max);
    let sorted = finite_probs(probs);
    log::info!(
        target: "vad",
        ch = ch,
        windows = stats.windows,
        speech_win = speech_win,
        speech_pct = if stats.windows == 0 { 0.0 } else { speech_win as f64 * 100.0 / stats.windows as f64 },
        silent_skip = stats.silent_skip,
        silent_infer = stats.silent_infer,
        resets = stats.resets,
        non_finite = stats.non_finite,
        p_min = percentile(&sorted, 0.0),
        p50 = percentile(&sorted, 0.5),
        p95 = percentile(&sorted, 0.95),
        p99 = percentile(&sorted, 0.99),
        p_max = percentile(&sorted, 1.0),
        segs = segments.len(),
        speech_sec = speech_sec,
        max_seg_sec = max_seg_sec;
        "ev=vad.summary"
    );
    let mut buckets = [0usize; 20];
    for &probability in probs {
        if probability.is_finite() {
            let index = ((probability.clamp(0.0, 1.0) * 19.0).round() as usize).min(19);
            buckets[index] += 1;
        }
    }
    log::debug!(
        target: "vad",
        ch = ch,
        b0 = buckets[0], b1 = buckets[1], b2 = buckets[2], b3 = buckets[3],
        b4 = buckets[4], b5 = buckets[5], b6 = buckets[6], b7 = buckets[7],
        b8 = buckets[8], b9 = buckets[9], b10 = buckets[10], b11 = buckets[11],
        b12 = buckets[12], b13 = buckets[13], b14 = buckets[14], b15 = buckets[15],
        b16 = buckets[16], b17 = buckets[17], b18 = buckets[18], b19 = buckets[19];
        "ev=vad.hist"
    );
    segments
}

pub(super) fn log_vad_segments(ch: i32, probs: &[f32], segments: &[(f32, f32)]) {
    if !log::log_enabled!(target: "vad", log::Level::Debug) {
        return;
    }
    let win_sec = VAD_WINDOW as f32 / 16000.0;
    for (index, &(start, end)) in segments.iter().enumerate() {
        let lo = (start / win_sec) as usize;
        let hi = ((end / win_sec).ceil() as usize).min(probs.len());
        let slice = probs.get(lo..hi).unwrap_or(&[]);
        let mean = if slice.is_empty() {
            0.0
        } else {
            slice.iter().map(|&p| p as f64).sum::<f64>() / slice.len() as f64
        };
        let max = slice.iter().copied().fold(0.0f32, f32::max);
        log::debug!(
            target: "vad",
            ch = ch,
            i = index,
            start_sec = start,
            end_sec = end,
            dur_sec = end - start,
            p_mean = mean,
            p_max = max;
            "ev=vad.segment"
        );
    }
}

pub(super) fn log_vad_windows(ch: i32, samples: &[f32], probs: &[f32]) {
    if !log::log_enabled!(target: "vad", log::Level::Trace) {
        return;
    }
    let mut emitted = 0usize;
    let mut suppressed = 0usize;
    for (index, &probability) in probs.iter().enumerate() {
        let interesting = (0.05..=0.6).contains(&probability);
        if index % 64 != 0 && !interesting {
            suppressed += 1;
            continue;
        }
        if emitted >= 2000 {
            suppressed += 1;
            continue;
        }
        let lo = index * VAD_WINDOW;
        let hi = (lo + VAD_WINDOW).min(samples.len());
        let stats = audio_stats(&samples[lo..hi]);
        log::trace!(
            target: "vad",
            ch = ch,
            i = index,
            t_sec = index as f64 * VAD_WINDOW as f64 / 16000.0,
            p = probability,
            rms = stats.rms,
            peak = stats.peak,
            action = if probability >= VAD_THRESHOLD { "speech" } else { "silence" };
            "ev=vad.window"
        );
        emitted += 1;
    }
    if suppressed > 0 {
        log::debug!(
            target: "vad",
            ch = ch,
            count = suppressed;
            "ev=vad.window.suppressed"
        );
    }
}

pub(super) fn mel_stats(mel: &[Vec<f32>]) -> (f32, f32, usize) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut non_finite = 0usize;
    for row in mel {
        for &value in row {
            if !value.is_finite() {
                non_finite += 1;
                continue;
            }
            min = min.min(value);
            max = max.max(value);
        }
    }
    if min.is_finite() {
        (min, max, non_finite)
    } else {
        (0.0, 0.0, non_finite)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct LoopMetrics {
    pub(super) words: usize,
    pub(super) max_run: usize,
    pub(super) unique_ratio: f32,
    pub(super) max_count: usize,
}

pub(super) fn asr_loop_metrics(text: &str) -> LoopMetrics {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        return LoopMetrics::default();
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut max_run = 1usize;
    let mut run = 1usize;
    for (i, w) in words.iter().enumerate() {
        *counts.entry(w.as_str()).or_default() += 1;
        if i > 0 {
            run = if words[i] == words[i - 1] { run + 1 } else { 1 };
            max_run = max_run.max(run);
        }
    }
    LoopMetrics {
        words: words.len(),
        max_run,
        unique_ratio: counts.len() as f32 / words.len() as f32,
        max_count: counts.values().copied().max().unwrap_or(0),
    }
}

pub(super) fn metrics_look_like_loop(metrics: LoopMetrics) -> bool {
    if metrics.words < 8 {
        return false;
    }
    metrics.max_run >= 8
        || (metrics.max_run >= 6 && metrics.unique_ratio < 0.4)
        || (metrics.max_count >= 10 && metrics.max_count as f32 / metrics.words as f32 >= 0.35)
        || (metrics.words >= 20 && metrics.unique_ratio < 0.22)
}

#[cfg(test)]
fn looks_like_asr_loop(text: &str) -> bool {
    metrics_look_like_loop(asr_loop_metrics(text))
}

pub(super) fn model_files(model_id: &str) -> Vec<serde_json::Value> {
    let dir = models_root().join(model_id);
    let mut files: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            Some(serde_json::json!({
                "id": model_id,
                "file": path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                "bytes": entry.metadata().map(|m| m.len()).unwrap_or(0),
            }))
        })
        .collect();
    files.sort_by(|a, b| {
        a["file"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["file"].as_str().unwrap_or_default())
    });
    files
}

#[cfg(test)]
mod tests {
    use super::looks_like_asr_loop;

    #[test]
    fn detects_repeated_asr_hallucination() {
        let looped = format!(
            "Проверка того, как пишутся{} пишется либо.",
            "-и".repeat(40)
        );
        assert!(looks_like_asr_loop(&looped));
        assert!(looks_like_asr_loop(
            "кстати, кстати, кстати, кстати, кстати, кстати, кстати, кстати, всё"
        ));
        assert!(looks_like_asr_loop(
            "Да-да-да-да-да-да-да-да-да-да-да-да, вот так вот оно и было"
        ));
    }

    #[test]
    fn keeps_normal_speech_with_small_repetitions() {
        assert!(!looks_like_asr_loop(
            "Проверка того, как пишется, пишется ли."
        ));
        assert!(!looks_like_asr_loop(
            "Да-да-да, хорошо, встретимся завтра в десять утра."
        ));
        assert!(!looks_like_asr_loop(""));
    }
}
