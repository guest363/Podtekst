use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;

use crate::transcribe::echo::{self, EchoEvidence};
use crate::transcribe::{Utterance, WordConf};

/// Разбивает длинные реплики в моно по естественным паузам тишины (RMS по окнам 20 мс,
/// паузы от ~140 мс) на фрагменты от 0.6 до 6 с (до 6.6 с, если паузы нет).
///
/// Зачем это нужно: в моно микрофон пишет обоих людей в один канал. Эмбеддинг
/// 20-секундного куска сплошной речи — это «средний голос» двух людей, и
/// кластеризация сливает весь разговор в одного спикера. Соседние куски
/// одного голоса потом снова склеиваются в [`merge_utterances`].
pub(crate) fn split_at_pauses(samples: &[f32], utterances: &[Utterance]) -> Vec<Utterance> {
    const FRAME_SEC: f64 = 0.02;
    const MIN_UNIT_SEC: f64 = 0.6;
    const PAUSE_MAX_UNIT: f64 = 6.0;
    const MIN_PAUSE_FRAMES: usize = 5; // 5 сглаженных кадров = тишина от ~140 мс в сыром звуке (отсекает смычки согласных 30–100 мс)
    let frame = (FRAME_SEC * 16000.0) as usize;
    let rms: Vec<f64> = samples
        .chunks(frame)
        .map(|c| {
            (c.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / c.len().max(1) as f64).sqrt()
        })
        .collect();
    if rms.len() < 3 {
        return utterances.to_vec();
    }
    // Сглаживание по 5 кадрам (~0.1 с): одиночный тихий отсчёт внутри слова
    // не должен резать фразу.
    let smooth: Vec<f64> = (0..rms.len())
        .map(|i| {
            let lo = i.saturating_sub(2);
            let hi = (i + 3).min(rms.len());
            rms[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
        })
        .collect();
    let mut sorted = rms.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    // Пауза заметно тише обычной речи. Порог — от медианы и шумового пола,
    // а не в абсолютных единицах: тихая и громкая записи режутся одинаково.
    let noise_floor = sorted[sorted.len() / 20];
    let speech_level = sorted[sorted.len() * 3 / 4];
    // Порог ниже обычной речи, но выше шумового пола: иначе на ровном
    // сигнале «паузой» станет сама речь и реплика рассыплется на куски.
    let quiet = (median * 0.35)
        .max(noise_floor * 3.0)
        .min(speech_level * 0.2);
    let mut out = Vec::with_capacity(utterances.len());
    for u in utterances {
        let mut bounds = vec![u.start];
        // Режем в естественных тихих паузах (от 0.6 с, паузы от ~140 мс): короткие реплики-ответы
        // («Ну да», «Хорошо») часто примыкают к чужой речи внутри одного VAD-окна.
        while *bounds.last().unwrap() + MIN_UNIT_SEC * 2.0 < u.end {
            let from = *bounds.last().unwrap();
            let lo = ((from + MIN_UNIT_SEC) / FRAME_SEC) as usize;
            let hi = (((from + PAUSE_MAX_UNIT) / FRAME_SEC) as usize + 1)
                .min(smooth.len())
                .min((u.end / FRAME_SEC) as usize);
            if lo >= hi {
                break;
            }
            let mut quiet_start = None;
            let mut cut = None;
            for i in lo..hi {
                if smooth[i] <= quiet {
                    if quiet_start.is_none() {
                        quiet_start = Some(i);
                    }
                } else if let Some(qs) = quiet_start {
                    if i - qs >= MIN_PAUSE_FRAMES {
                        let best_i = (qs..i)
                            .min_by(|&a, &b| smooth[a].total_cmp(&smooth[b]))
                            .unwrap();
                        let cut_time = best_i as f64 * FRAME_SEC;
                        if cut_time + MIN_UNIT_SEC <= u.end && cut_time >= from + MIN_UNIT_SEC {
                            cut = Some(cut_time);
                            break;
                        }
                    }
                    quiet_start = None;
                }
            }
            if cut.is_none() {
                if let Some(qs) = quiet_start {
                    if hi - qs >= MIN_PAUSE_FRAMES {
                        let best_i = (qs..hi)
                            .min_by(|&a, &b| smooth[a].total_cmp(&smooth[b]))
                            .unwrap();
                        let cut_time = best_i as f64 * FRAME_SEC;
                        if cut_time + MIN_UNIT_SEC <= u.end && cut_time >= from + MIN_UNIT_SEC {
                            cut = Some(cut_time);
                        }
                    }
                }
            }
            let cut = match cut {
                Some(c) if c > from + 0.2 => c,
                _ => {
                    if from + PAUSE_MAX_UNIT + MIN_UNIT_SEC <= u.end {
                        from + PAUSE_MAX_UNIT
                    } else {
                        break;
                    }
                }
            };
            if cut <= from + 0.2 {
                break;
            }
            bounds.push(cut);
        }
        bounds.push(u.end);
        let n = bounds.len() - 1;
        if n < 2 {
            out.push(u.clone());
            continue;
        }
        let parts = split_utterance_by_bounds(u, &bounds);
        if parts.is_empty() {
            out.push(u.clone());
        } else {
            out.extend(parts);
        }
    }
    out
}

pub(crate) fn split_by_word_pauses(utterances: &[Utterance], min_gap_sec: f64) -> Vec<Utterance> {
    const MIN_UNIT_SEC: f64 = 1.0;
    let mut out = Vec::with_capacity(utterances.len());
    for u in utterances {
        if u.words.is_empty() || u.end - u.start < MIN_UNIT_SEC * 2.0 {
            out.push(u.clone());
            continue;
        }
        let timed: Vec<(f64, f64)> = u
            .words
            .iter()
            .filter(|w| w.end_sec > w.start_sec)
            .map(|w| (w.start_sec, w.end_sec))
            .collect();
        if timed.len() < 2 {
            out.push(u.clone());
            continue;
        }
        let mut bounds = vec![u.start];
        for w in timed.windows(2) {
            let gap = w[1].0 - w[0].1;
            if gap >= min_gap_sec {
                let cut = (w[0].1 + w[1].0) / 2.0;
                let from = *bounds.last().unwrap();
                if cut - from >= MIN_UNIT_SEC && u.end - cut >= MIN_UNIT_SEC {
                    bounds.push((cut * 100.0).round() / 100.0);
                }
            }
        }
        if bounds.len() <= 1 {
            out.push(u.clone());
            continue;
        }
        bounds.push(u.end);
        let parts = split_utterance_by_bounds(u, &bounds);
        if parts.is_empty() {
            out.push(u.clone());
        } else {
            out.extend(parts);
        }
    }
    out
}

/// Эмбеддинг на ARM стоит сотни миллисекунд: короткие реплики не режем,
/// число окон ограничиваем.
const SPLIT_MIN_DUR: f64 = 3.0;
const SPLIT_MAX_WINDOWS: usize = 8;
const SPLIT_WINDOW_SEC: f64 = 1.6;

/// Шаг скользящего окна: на коротких репликах 0.5 с, на длинных растягивается
/// так, чтобы уложиться в [`SPLIT_MAX_WINDOWS`].
fn split_step(duration: f64) -> f64 {
    let span = (duration - SPLIT_WINDOW_SEC).max(0.0);
    (span / (SPLIT_MAX_WINDOWS - 1) as f64).max(0.5)
}

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

pub(super) fn speaker_change_cuts(
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

pub(super) fn split_window_region(
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
        let mut wins: Vec<(f64, Vec<f64>)> = vec![];
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
                        match ext.embed(&chunk[..clean_len]) {
                            Ok(Some(e)) => wins.push((t, e)),
                            Ok(None) => {}
                            Err(error) => log::warn!(
                                target: "diar",
                                i = index,
                                error = error.to_string();
                                "ev=diar.embed_error"
                            ),
                        }
                    }
                }
            }
            t += step;
        }
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

pub(crate) fn split_utterance_by_bounds(u: &Utterance, bounds: &[f64]) -> Vec<Utterance> {
    if bounds.len() < 2 {
        return vec![u.clone()];
    }
    let words: Vec<&str> = u.text.split_whitespace().collect();
    if words.is_empty() {
        return bounds
            .windows(2)
            .map(|pair| Utterance {
                start: pair[0],
                end: pair[1],
                ..u.clone()
            })
            .collect();
    }

    let has_timed_words = !u.words.is_empty() && u.words.iter().any(|w| w.end_sec > w.start_sec);

    if has_timed_words {
        let word_bounds =
            crate::transcribe::word_speakers::resolve_word_bounds(&words, &u.words, u.start, u.end);
        let n_segs = bounds.len() - 1;
        let mut seg_words: Vec<Vec<WordConf>> = vec![Vec::new(); n_segs];
        for (i, &w_text) in words.iter().enumerate() {
            let (s, e) = word_bounds[i];
            let mid = (s + e) / 2.0;
            let mut seg_idx = n_segs - 1;
            for (idx, w) in bounds.windows(2).enumerate() {
                if mid < w[1] || idx == n_segs - 1 {
                    seg_idx = idx;
                    break;
                }
            }
            let wc = if let Some(mut w) = u.words.get(i).cloned() {
                if w.end_sec <= w.start_sec {
                    w.start_sec = (s * 1000.0).round() / 1000.0;
                    w.end_sec = (e * 1000.0).round() / 1000.0;
                }
                w
            } else {
                WordConf {
                    w: w_text.to_string(),
                    conf: 1.0,
                    needs_review: false,
                    start_sec: (s * 1000.0).round() / 1000.0,
                    end_sec: (e * 1000.0).round() / 1000.0,
                }
            };
            seg_words[seg_idx].push(wc);
        }

        let mut parts = Vec::new();
        for (idx, w) in bounds.windows(2).enumerate() {
            let words = &seg_words[idx];
            if words.is_empty() {
                continue;
            }
            let text = words
                .iter()
                .map(|w| w.w.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            parts.push(Utterance {
                start: (w[0] * 100.0).round() / 100.0,
                end: (w[1] * 100.0).round() / 100.0,
                speaker: u.speaker.clone(),
                channel: u.channel,
                text,
                diarized: false,
                silhouette: 0.0,
                words: words.clone(),
                ctc_used: u.ctc_used,
            });
        }
        if !parts.is_empty() {
            return parts;
        }
    }

    let num_segments = bounds.len() - 1;
    if words.len() < num_segments {
        return vec![u.clone()];
    }
    let mut parts = Vec::new();
    let mut wpos = 0;
    for (text, w) in split_words_by_bounds(&words, bounds)
        .into_iter()
        .zip(bounds.windows(2))
    {
        if text.is_empty() {
            continue;
        }
        let nw = text.split_whitespace().count();
        let wconfs = u.words.get(wpos..wpos + nw).unwrap_or(&[]).to_vec();
        wpos += nw;
        parts.push(Utterance {
            start: (w[0] * 100.0).round() / 100.0,
            end: (w[1] * 100.0).round() / 100.0,
            speaker: u.speaker.clone(),
            channel: u.channel,
            text,
            diarized: false,
            silhouette: 0.0,
            words: wconfs,
            ctc_used: u.ctc_used,
        });
    }
    parts
}

/// Раскладка слов реплики по окнам между границами (доля окна от общей
/// длительности ≈ доля слов). Правая граница среза — от длины массива,
/// а не остатка: при `lo > words.len()/2` остаток даёт срез за границами массива.
pub(crate) fn split_words_by_bounds(words: &[&str], bounds: &[f64]) -> Vec<String> {
    let total: f64 = bounds
        .windows(2)
        .map(|w| w[1] - w[0])
        .sum::<f64>()
        .max(1e-6);
    let mut texts = Vec::with_capacity(bounds.len().saturating_sub(1));
    let mut pos = 0usize;
    for (k, w) in bounds.windows(2).enumerate() {
        let frac = (w[1] - w[0]) / total;
        let nw = if k + 1 == bounds.len() - 1 {
            // Предыдущие ветки резервируют по слову на каждый оставшийся
            // фрагмент: при words.len() >= числа фрагментов остаток не пуст
            // и последний интервал не пропадает из таймлайна.
            words.len().saturating_sub(pos)
        } else {
            let remaining_segs = bounds.len() - 1 - (k + 1);
            let max_nw = words.len().saturating_sub(pos + remaining_segs).max(1);
            let raw_nw = ((words.len() as f64 * frac).round() as usize).clamp(1, max_nw);
            let mut snapped = raw_nw;
            let target = pos + raw_nw;
            let is_punct = |s: &str| s.ends_with(['.', '!', '?', ',', ';', ':']);
            if target > pos && target <= words.len() && is_punct(words[target - 1]) {
            } else if target > pos + 1 && is_punct(words[target - 2]) {
                snapped = (raw_nw - 1).clamp(1, max_nw);
            } else if target < words.len() && is_punct(words[target]) && raw_nw < max_nw {
                snapped = (raw_nw + 1).clamp(1, max_nw);
            }
            snapped
        };
        let lo = pos.min(words.len());
        let hi = (lo + nw).min(words.len());
        texts.push(words[lo..hi].join(" "));
        pos += nw;
    }
    texts
}
