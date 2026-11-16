use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;

use crate::transcribe::echo::{self, EchoEvidence};
use crate::transcribe::{Utterance, WordConf};

use super::bounds::split_utterance_by_bounds;
use super::pauses::{split_step, SPLIT_MIN_DUR, SPLIT_WINDOW_SEC};

/// Веса выбора разреза: тишина решает, длинная пауза перевешивает смещение
/// от середины, а кадру без длительности паузы смещение стоит дешевле.
const CUT_SCORE_RMS: f64 = 10.0;
const CUT_SCORE_PAUSE: f64 = 2.0;
const CUT_SCORE_WORD_DIST: f64 = 1.5;
const CUT_SCORE_FRAME_DIST: f64 = 0.5;

fn find_speaker_cut_point(
    samples: &[f32],
    start: f64,
    end: f64,
    w_start_a: f64,
    w_start_b: f64,
    words: &[WordConf],
) -> f64 {
    let search_lo = (w_start_a - 0.2).max(start);
    let search_hi = (w_start_b + SPLIT_WINDOW_SEC + 0.2).min(end);
    let mid_expected = (w_start_a + w_start_b + SPLIT_WINDOW_SEC) / 2.0;

    let timed: Vec<(f64, f64)> = words
        .iter()
        .filter(|w| w.end_sec > w.start_sec)
        .map(|w| (w.start_sec, w.end_sec))
        .collect();

    let sr = 16000.0;
    let frame_len = (0.05 * sr) as usize;

    let rms_at = |time_sec: f64| -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let center = (time_sec * sr) as usize;
        let half = frame_len / 2;
        let lo = center.saturating_sub(half);
        let hi = (center + half).min(samples.len());
        if hi <= lo {
            return 0.0;
        }
        let slice = &samples[lo..hi];
        let sum_sq: f64 = slice.iter().map(|&x| (x as f64) * (x as f64)).sum();
        (sum_sq / slice.len() as f64).sqrt()
    };

    if !timed.is_empty() {
        let mut best_cut = None;
        let mut best_score = f64::MAX;

        for w in timed.windows(2) {
            let gap_start = w[0].1;
            let gap_end = w[1].0;
            let cut_t = (gap_start + gap_end) / 2.0;
            if cut_t >= search_lo && cut_t <= search_hi {
                let rms = rms_at(cut_t);
                let pause = (gap_end - gap_start).max(0.0);
                let dist = (cut_t - mid_expected).abs();
                let score =
                    rms * CUT_SCORE_RMS - pause * CUT_SCORE_PAUSE + dist * CUT_SCORE_WORD_DIST;
                if score < best_score {
                    best_score = score;
                    best_cut = Some(cut_t);
                }
            }
        }
        if let Some(cut) = best_cut {
            return cut.clamp(start + 1.0, end - 1.0);
        }
    }

    let lo = (search_lo * sr) as usize;
    let hi = (search_hi * sr) as usize;
    if hi > lo + frame_len && hi <= samples.len() {
        let mut min_score = f64::INFINITY;
        let mut best_sec = mid_expected;
        for chunk_start in (lo..hi - frame_len).step_by(frame_len / 2) {
            let slice = &samples[chunk_start..chunk_start + frame_len];
            let sum_sq: f64 = slice.iter().map(|&x| (x as f64) * (x as f64)).sum();
            let rms = (sum_sq / slice.len() as f64).sqrt();
            let sec = (chunk_start + frame_len / 2) as f64 / sr;
            let dist = (sec - mid_expected).abs();
            let score = rms * CUT_SCORE_RMS + dist * CUT_SCORE_FRAME_DIST;
            if score < min_score {
                min_score = score;
                best_sec = sec;
            }
        }
        return best_sec.clamp(start + 1.0, end - 1.0);
    }

    mid_expected.clamp(start + 1.0, end - 1.0)
}

pub(crate) fn speaker_change_cuts(
    samples: &[f32],
    start: f64,
    end: f64,
    wins: &[(f64, Vec<f64>)],
    words: &[WordConf],
) -> Vec<f64> {
    let sims: Vec<f64> = wins
        .windows(2)
        .map(|w| mr_ml::cosine_similarity(&w[0].1, &w[1].1))
        .collect();
    let mut cuts = vec![];
    for (i, &s) in sims.iter().enumerate() {
        if s >= 0.35 {
            continue;
        }
        let prev = if i > 0 { sims[i - 1] } else { 1.0 };
        let next = sims.get(i + 1).copied().unwrap_or(1.0);
        if s > prev || s > next {
            continue;
        }
        let b = find_speaker_cut_point(samples, start, end, wins[i].0, wins[i + 1].0, words);
        if cuts.last().map(|&c| b - c >= 1.0).unwrap_or(true) {
            cuts.push((b * 100.0).round() / 100.0);
        }
    }
    if wins.len() >= 3 {
        let sims2: Vec<f64> = (0..wins.len() - 2)
            .map(|i| mr_ml::cosine_similarity(&wins[i].1, &wins[i + 2].1))
            .collect();
        for (i, &s2) in sims2.iter().enumerate() {
            if s2 >= 0.35 {
                continue;
            }
            let prev = if i > 0 { sims2[i - 1] } else { 1.0 };
            let next = sims2.get(i + 1).copied().unwrap_or(1.0);
            if s2 > prev || s2 > next {
                continue;
            }
            let b = find_speaker_cut_point(samples, start, end, wins[i].0, wins[i + 2].0, words);
            let b_rounded = (b * 100.0).round() / 100.0;
            if cuts.iter().all(|&c| (b_rounded - c).abs() >= 1.0) {
                cuts.push(b_rounded);
            }
        }
        cuts.sort_by(|a, b| a.total_cmp(b));
    }
    cuts
}

/// Окно поиска смены голоса должно содержать хотя бы столько чистой локальной
/// речи: иначе эмбеддинг окна — это голос из динамиков, а не собеседника.
const SPLIT_MIN_LOCAL_SEC: f64 = 0.8;
/// Окно, больше половины которого — только эхо, для сравнения голосов не годится.
const SPLIT_MAX_ECHO_SHARE: f32 = 0.5;

/// Участок окна без «только эхо», обрезанный по границам окна: `None` — кадров
/// локальной речи нет или их слишком мало для честного эмбеддинга.
pub(crate) fn local_region(
    ev: &EchoEvidence,
    start: f64,
    end: f64,
    min_sec: f64,
) -> Option<(f64, f64)> {
    let (run_start, run_end) = echo::longest_local_run(ev, start, end)?;
    let run_start = run_start.max(start);
    let run_end = run_end.min(end);
    (run_end - run_start >= min_sec).then_some((run_start, run_end))
}

pub(crate) fn split_window_region(
    echo: Option<&EchoEvidence>,
    start: f64,
    end: f64,
) -> Option<(f64, f64)> {
    match echo {
        Some(ev) => {
            if echo::segment_profile(ev, start, end).echo_only > SPLIT_MAX_ECHO_SHARE {
                return None;
            }
            local_region(ev, start, end, SPLIT_MIN_LOCAL_SEC)
        }
        None => Some((start, end)),
    }
}

/// Один голос даёт ~0.5+, смена диктора ≤ 0.22 — замеры ERes2NetV2
/// (`onnx_real::eres2netv2_parity_with_official_reference`).
/// Перекрытия вырезаются до эмбеддинга: иначе оба голоса дают «средний» вектор.
pub(crate) fn split_long_by_speaker_change(
    samples: &[f32],
    utterances: Vec<Utterance>,
    extractor: &mut Option<EmbeddingExtractor>,
    segmentation: Option<&Segmentation>,
    echo: Option<&EchoEvidence>,
    on_event: &dyn Fn(u8, &str),
) -> Vec<Utterance> {
    let Some(ext) = extractor else {
        return utterances;
    };
    let sr = 16000.0;
    let mut out = Vec::new();
    let total = utterances.len();
    for (index, u) in utterances.into_iter().enumerate() {
        if index % 5 == 0 || index + 1 == total {
            on_event(
                80,
                &format!("Ищу смену голоса: реплика {} из {total}", index + 1),
            );
        }
        let dur = u.end - u.start;
        if dur < SPLIT_MIN_DUR {
            out.push(u);
            continue;
        }
        let step = split_step(dur);
        let mut chunks: Vec<(f64, Vec<f32>)> = Vec::new();
        let mut t = u.start;
        while t + SPLIT_WINDOW_SEC <= u.end + 1e-6 {
            let window_end = (t + SPLIT_WINDOW_SEC).min(u.end);
            if let Some((start, end)) = split_window_region(echo, t, window_end) {
                let lo = (start * sr) as usize;
                let hi = (end * sr) as usize;
                if hi > lo && hi <= samples.len() {
                    let mut chunk = samples[lo..hi].to_vec();
                    let clean_len = segmentation
                        .map(|seg| seg.exclude_overlap(&mut chunk, lo))
                        .unwrap_or(chunk.len());
                    if clean_len >= (sr * 0.4) as usize {
                        chunk.truncate(clean_len);
                        chunks.push((t, chunk));
                    }
                }
            }
            t += step;
        }
        let refs: Vec<&[f32]> = chunks.iter().map(|(_, chunk)| chunk.as_slice()).collect();
        let wins: Vec<(f64, Vec<f64>)> = match ext.embed_batch(&refs) {
            Ok(embeddings) => chunks
                .into_iter()
                .zip(embeddings)
                .filter_map(|((start, _), embedding)| embedding.map(|embedding| (start, embedding)))
                .collect(),
            Err(error) => {
                log::warn!(
                    target: "diar",
                    i = index,
                    error = error.to_string();
                    "ev=diar.embed_error"
                );
                chunks
                    .iter()
                    .filter_map(|(start, chunk)| {
                        ext.embed(chunk)
                            .ok()
                            .flatten()
                            .map(|embedding| (*start, embedding))
                    })
                    .collect()
            }
        };
        if wins.len() < 3 {
            log::debug!(
                target: "diar",
                i = index,
                dur_sec = dur,
                windows = wins.len(),
                cuts = 0,
                reason = "too_few_windows";
                "ev=diar.split"
            );
            out.push(u);
            continue;
        }
        let cuts = speaker_change_cuts(samples, u.start, u.end, &wins, &u.words);
        log::debug!(
            target: "diar",
            i = index,
            dur_sec = dur,
            windows = wins.len(),
            cuts = cuts.len(),
            reason = if cuts.is_empty() { "no_change" } else { "split" };
            "ev=diar.split"
        );
        if cuts.is_empty() {
            out.push(u);
            continue;
        }
        let mut bounds = vec![u.start];
        bounds.extend(cuts);
        bounds.push(u.end);
        let parts = split_utterance_by_bounds(&u, &bounds);
        if parts.is_empty() {
            out.push(u);
        } else {
            out.extend(parts);
        }
    }
    out
}
