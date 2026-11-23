use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;

use crate::transcribe::echo::{self, EchoEvidence};
use crate::transcribe::{Utterance, WordConf};

use super::bounds::split_utterance_by_bounds;
use super::pauses::{split_step, word_boundaries, SPLIT_MIN_DUR, SPLIT_WINDOW_SEC};

/// Веса выбора разреза: тишина решает, длинная пауза и конец предложения / тире
/// реплики перевешивают смещение от середины, а внутрифразный зазор < 120 мс
/// штрафуется, чтобы разрез не попадал на случайную смычку внутри фразы.
const CUT_SCORE_RMS: f64 = 10.0;
const CUT_SCORE_PAUSE: f64 = 2.0;
const CUT_SCORE_WORD_DIST: f64 = 1.5;
const CUT_SCORE_FRAME_DIST: f64 = 0.5;
const CUT_SCORE_SENTENCE_END: f64 = 0.85;
const CUT_SCORE_TURN_DASH: f64 = 1.0;
const CUT_SCORE_MID_PHRASE_TINY_GAP: f64 = 0.65;
const TINY_MID_PHRASE_GAP_SEC: f64 = 0.12;
const CUT_SAME_SPEAKER_MIN_SIM: f64 = 0.35;
// Поштучный срез по точке не опирается на 3+ окон, поэтому требуем контраст
// строже обычного порога и не режем, если срез попадает в плотное перекрытие.
const SENTENCE_CUT_SAME_SPEAKER_MIN_SIM: f64 = 0.30;
const SENTENCE_CUT_OVERLAP_GUARD_SEC: f64 = 0.75;
const SENTENCE_CUT_MAX_LOCAL_OVERLAP_SHARE: f64 = 0.20;
const SENTENCE_CUT_MAX_ARM_OVERLAP_SHARE: f64 = 0.15;
const MIN_SENTENCE_SPLIT_SIDE_SEC: f64 = 1.0;

fn lowest_rms_time(samples: &[f32], search_lo: f64, search_hi: f64, fallback: f64) -> f64 {
    let sr = 16000.0;
    let frame_len = (0.05 * sr) as usize;
    let lo = (search_lo * sr) as usize;
    let hi = (search_hi * sr) as usize;
    if hi <= lo + frame_len || hi > samples.len() {
        return fallback;
    }
    let mut min_score = f64::INFINITY;
    let mut best_sec = fallback;
    for chunk_start in (lo..hi - frame_len).step_by(frame_len / 2) {
        let slice = &samples[chunk_start..chunk_start + frame_len];
        let sum_sq: f64 = slice.iter().map(|&x| (x as f64) * (x as f64)).sum();
        let rms = (sum_sq / slice.len() as f64).sqrt();
        let sec = (chunk_start + frame_len / 2) as f64 / sr;
        let score = rms * CUT_SCORE_RMS + (sec - fallback).abs() * CUT_SCORE_FRAME_DIST;
        if score < min_score {
            min_score = score;
            best_sec = sec;
        }
    }
    best_sec
}

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

    let boundaries = word_boundaries(words, start, end);
    if !boundaries.is_empty() {
        let mut best_cut = None;
        let mut best_score = f64::MAX;

        for b in boundaries {
            if b.cut_t >= search_lo && b.cut_t <= search_hi {
                let rms = rms_at(b.cut_t);
                let dist = (b.cut_t - mid_expected).abs();
                let mut score =
                    rms * CUT_SCORE_RMS - b.pause * CUT_SCORE_PAUSE + dist * CUT_SCORE_WORD_DIST;
                if b.ends_sentence {
                    score -= CUT_SCORE_SENTENCE_END;
                }
                if b.starts_turn_dash {
                    score -= CUT_SCORE_TURN_DASH;
                }
                if !b.ends_sentence && !b.starts_turn_dash && b.pause < TINY_MID_PHRASE_GAP_SEC {
                    score += CUT_SCORE_MID_PHRASE_TINY_GAP;
                }
                if score < best_score {
                    best_score = score;
                    best_cut = Some(b.cut_t);
                }
            }
        }
        if let Some(cut) = best_cut {
            return cut.clamp(start + 1.0, end - 1.0);
        }
    }

    lowest_rms_time(samples, search_lo, search_hi, mid_expected).clamp(start + 1.0, end - 1.0)
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
        if s >= CUT_SAME_SPEAKER_MIN_SIM {
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
            if s2 >= CUT_SAME_SPEAKER_MIN_SIM {
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

fn extract_clean_slice(
    samples: &[f32],
    start: f64,
    end: f64,
    segmentation: Option<&Segmentation>,
    echo: Option<&EchoEvidence>,
) -> Option<Vec<f32>> {
    let sr = 16000.0;
    let (region_start, region_end) = split_window_region(echo, start, end)?;
    let lo = (region_start * sr) as usize;
    let hi = (region_end * sr) as usize;
    if hi <= lo || hi > samples.len() {
        return None;
    }
    let mut chunk = samples[lo..hi].to_vec();
    let clean_len = segmentation
        .map(|seg| seg.exclude_overlap(&mut chunk, lo))
        .unwrap_or(chunk.len());
    if clean_len < (sr * 0.4) as usize {
        return None;
    }
    chunk.truncate(clean_len);
    Some(chunk)
}

fn sentence_cut_candidates(samples: &[f32], u: &Utterance) -> Vec<f64> {
    if u.words.len() < 2 || u.end - u.start < 2.0 * MIN_SENTENCE_SPLIT_SIDE_SEC {
        return Vec::new();
    }
    let boundaries = word_boundaries(&u.words, u.start, u.end);
    if !boundaries.iter().any(|b| b.ends_sentence) {
        return Vec::new();
    }
    let texts: Vec<&str> = u.text.split_whitespace().collect();
    if texts.len() < 2 {
        return Vec::new();
    }
    let word_bounds =
        crate::transcribe::word_speakers::resolve_word_bounds(&texts, &u.words, u.start, u.end);
    let mut candidates = Vec::new();
    let mut prev = u.start;
    for (k, b) in boundaries.iter().enumerate() {
        if !b.ends_sentence || k + 1 >= word_bounds.len() {
            continue;
        }
        let left_mid = f64::midpoint(word_bounds[k].0, word_bounds[k].1);
        let right_mid = f64::midpoint(word_bounds[k + 1].0, word_bounds[k + 1].1);
        let lo = (b.cut_t - 0.35)
            .max(prev + MIN_SENTENCE_SPLIT_SIDE_SEC)
            .max(left_mid + 0.01);
        let hi = (b.cut_t + 0.15)
            .min(u.end - MIN_SENTENCE_SPLIT_SIDE_SEC)
            .min(right_mid - 0.01);
        if lo >= hi {
            continue;
        }
        let snapped = lowest_rms_time(samples, lo, hi, b.cut_t.clamp(lo, hi));
        let cut = (snapped * 100.0).round() / 100.0;
        if cut - prev >= MIN_SENTENCE_SPLIT_SIDE_SEC && u.end - cut >= MIN_SENTENCE_SPLIT_SIDE_SEC {
            candidates.push(cut);
            prev = cut;
        }
    }
    candidates
}

fn sentence_cut_near_overlap(
    segmentation: Option<&Segmentation>,
    seg_start: f64,
    cut: f64,
    seg_end: f64,
) -> bool {
    let Some(seg) = segmentation else {
        return false;
    };
    let sr = 16000.0;
    let to_sample = |t: f64| (t.max(0.0) * sr) as usize;
    let guard_lo = to_sample((cut - SENTENCE_CUT_OVERLAP_GUARD_SEC).max(seg_start));
    let guard_hi = to_sample((cut + SENTENCE_CUT_OVERLAP_GUARD_SEC).min(seg_end));
    if seg.overlap_share(guard_lo, guard_hi) > SENTENCE_CUT_MAX_LOCAL_OVERLAP_SHARE {
        return true;
    }
    seg.overlap_share(to_sample(seg_start), to_sample(cut)) > SENTENCE_CUT_MAX_ARM_OVERLAP_SHARE
        || seg.overlap_share(to_sample(cut), to_sample(seg_end))
            > SENTENCE_CUT_MAX_ARM_OVERLAP_SHARE
}

/// Скользящее 1.6 с окно проглатывает смену голоса при перепаде громкости:
/// громкий сосед заглушает тихую половину. Граница предложения (`.`/`!`/`?`/`…`)
/// позволяет сравнить левый и правый куски порознь без смешивания энергий.
fn sentence_boundary_cuts(
    samples: &[f32],
    u: &Utterance,
    ext: &mut EmbeddingExtractor,
    segmentation: Option<&Segmentation>,
    echo: Option<&EchoEvidence>,
) -> Vec<f64> {
    let candidates = sentence_cut_candidates(samples, u);
    let mut cuts = Vec::new();
    let mut seg_start = u.start;
    for (idx, &cut) in candidates.iter().enumerate() {
        let seg_end = candidates.get(idx + 1).copied().unwrap_or(u.end);
        if sentence_cut_near_overlap(segmentation, seg_start, cut, seg_end) {
            continue;
        }
        let (Some(left), Some(right)) = (
            extract_clean_slice(samples, seg_start, cut, segmentation, echo),
            extract_clean_slice(samples, cut, seg_end, segmentation, echo),
        ) else {
            continue;
        };
        let Ok(embs) = ext.embed_batch(&[left.as_slice(), right.as_slice()]) else {
            continue;
        };
        if let [Some(l), Some(r)] = embs.as_slice() {
            if mr_ml::cosine_similarity(l, r) < SENTENCE_CUT_SAME_SPEAKER_MIN_SIM {
                cuts.push(cut);
                seg_start = cut;
            }
        }
    }
    cuts
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
        if dur < 2.0 * MIN_SENTENCE_SPLIT_SIDE_SEC {
            out.push(u);
            continue;
        }
        let mut wins: Vec<(f64, Vec<f64>)> = Vec::new();
        let mut cuts: Vec<f64> = Vec::new();
        if dur >= SPLIT_MIN_DUR {
            let step = split_step(dur);
            let mut chunks: Vec<(f64, Vec<f32>)> = Vec::new();
            let mut t = u.start;
            while t + SPLIT_WINDOW_SEC <= u.end + 1e-6 {
                let window_end = (t + SPLIT_WINDOW_SEC).min(u.end);
                if let Some(chunk) = extract_clean_slice(samples, t, window_end, segmentation, echo)
                {
                    chunks.push((t, chunk));
                }
                t += step;
            }
            let refs: Vec<&[f32]> = chunks.iter().map(|(_, chunk)| chunk.as_slice()).collect();
            wins = match ext.embed_batch(&refs) {
                Ok(embeddings) => chunks
                    .into_iter()
                    .zip(embeddings)
                    .filter_map(|((start, _), embedding)| {
                        embedding.map(|embedding| (start, embedding))
                    })
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
            if wins.len() >= 3 {
                cuts = speaker_change_cuts(samples, u.start, u.end, &wins, &u.words);
            }
        }
        if cuts.is_empty() {
            cuts = sentence_boundary_cuts(samples, &u, ext, segmentation, echo);
        }
        if dur >= SPLIT_MIN_DUR || !cuts.is_empty() {
            let reason = if !cuts.is_empty() {
                "split"
            } else if wins.len() < 3 {
                "too_few_windows"
            } else {
                "no_change"
            };
            log::debug!(
                target: "diar",
                i = index,
                dur_sec = dur,
                windows = wins.len(),
                cuts = cuts.len(),
                reason = reason;
                "ev=diar.split"
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn word(w: &str, start_sec: f64, end_sec: f64) -> WordConf {
        WordConf {
            w: w.to_string(),
            conf: 0.98,
            needs_review: false,
            start_sec,
            end_sec,
        }
    }

    #[test]
    fn sentence_boundary_and_turn_dash_win_over_closer_mid_phrase_gap() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let wins = vec![(0.0, a.clone()), (1.0, a), (2.0, b.clone()), (3.0, b)];
        let samples = vec![0.01f32; 16000 * 5];

        let sentence_words = vec![
            word("слово1", 0.3, 2.28),
            word("слово2?", 2.32, 2.86),
            word("слово3", 2.94, 4.6),
        ];
        let cuts_sentence = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &sentence_words);
        assert_eq!(cuts_sentence, vec![2.9]);

        let dash_words = vec![
            word("слово1", 0.3, 2.28),
            word("слово2.", 2.32, 2.86),
            word("—", 2.90, 2.95),
            word("слово3", 3.00, 4.6),
        ];
        let cuts_dash = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &dash_words);
        assert_eq!(cuts_dash, vec![2.88]);
    }

    #[test]
    fn standalone_dash_after_ellipsis_and_prefixed_dash_stay_in_right_half() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let wins = vec![(0.0, a.clone()), (1.0, a), (2.0, b.clone()), (3.0, b)];
        let samples = vec![0.01f32; 16000 * 5];

        let words_untimed = vec![
            word("слово1", 0.3, 1.8),
            word("...", 0.0, 0.0),
            word("—", 0.0, 0.0),
            word("слово2", 2.0, 2.28),
            word("слово3", 2.32, 4.6),
        ];
        let cuts = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &words_untimed);
        assert_eq!(cuts.len(), 1);
        let utt = Utterance {
            start: 0.0,
            end: 5.0,
            speaker: "X".to_string(),
            channel: Some(1),
            text: "слово1 ... — слово2 слово3".to_string(),
            diarized: false,
            silhouette: 0.0,
            words: words_untimed,
            ctc_used: true,
        };
        let parts = split_utterance_by_bounds(&utt, &[0.0, cuts[0], 5.0]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text, "слово1 ...");
        assert_eq!(parts[1].text, "— слово2 слово3");

        let words_prefixed = vec![
            word("слово1", 0.3, 2.28),
            word("слово2.", 2.32, 2.86),
            word("—...", 2.92, 3.05),
            word("слово3", 3.10, 4.6),
        ];
        let cuts_prefixed = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &words_prefixed);
        assert_eq!(cuts_prefixed, vec![2.89]);
    }

    #[test]
    fn sentence_cut_candidates_snap_to_pause_without_crossing_words() {
        let mut samples = vec![0.2f32; 16000 * 4];
        for x in samples[16000 * 13 / 10..16000 * 145 / 100].iter_mut() {
            *x = 0.001;
        }
        let u = Utterance {
            start: 0.0,
            end: 3.1,
            speaker: "X".to_string(),
            channel: None,
            text: "альфа? бета.".to_string(),
            diarized: false,
            silhouette: 0.0,
            words: vec![word("альфа?", 0.1, 1.45), word("бета.", 1.60, 3.0)],
            ctc_used: true,
        };
        let cuts = sentence_cut_candidates(&samples, &u);
        assert_eq!(cuts.len(), 1);
        assert!((1.25..=1.50).contains(&cuts[0]), "{cuts:?}");
        let parts = split_utterance_by_bounds(&u, &[0.0, cuts[0], 3.1]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text, "альфа?");
        assert_eq!(parts[1].text, "бета.");
    }

    #[test]
    fn sentence_cut_is_suppressed_near_overlap_region() {
        let frames_per_4s = (4 * 16000) / mr_ml::onnx::segmentation::FRAME_SAMPLES + 1;
        let mut frames = vec![1u8; frames_per_4s];
        let ov_lo = (11 * 1600) / mr_ml::onnx::segmentation::FRAME_SAMPLES;
        let ov_hi = (19 * 1600) / mr_ml::onnx::segmentation::FRAME_SAMPLES;
        for f in &mut frames[ov_lo..ov_hi] {
            *f = 4;
        }
        let seg = Segmentation {
            frames,
            windows: vec![],
        };
        assert!(sentence_cut_near_overlap(Some(&seg), 0.0, 1.5, 3.1));
        assert!(!sentence_cut_near_overlap(None, 0.0, 1.5, 3.1));

        let mut far_frames = vec![1u8; frames_per_4s * 3];
        if let Some(last) = far_frames.last_mut() {
            *last = 4;
        }
        let far_seg = Segmentation {
            frames: far_frames,
            windows: vec![],
        };
        assert!(!sentence_cut_near_overlap(Some(&far_seg), 0.0, 1.5, 3.1));
    }
}
