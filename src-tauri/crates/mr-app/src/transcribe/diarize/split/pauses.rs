use crate::transcribe::{Utterance, WordConf};

use super::bounds::split_utterance_by_bounds;

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

#[derive(Debug, Clone, Copy)]
pub(super) struct WordBoundary {
    pub cut_t: f64,
    pub pause: f64,
    pub ends_sentence: bool,
    pub starts_turn_dash: bool,
}

pub(super) fn ends_sentence(word: &str) -> bool {
    word.ends_with(['.', '!', '?', '…'])
}

pub(super) fn starts_turn_dash(word: &str) -> bool {
    word.starts_with('—')
}

/// Строим границы между всеми словами, а не только с ненулевыми таймкодами:
/// при CTC-выравнивании одиночные токены пунктуации (`…`, `—`) часто остаются
/// без времени (`0.0, 0.0`), и пропуск скрыл бы смену реплики и реальную паузу.
pub(super) fn word_boundaries(
    words: &[WordConf],
    utt_start: f64,
    utt_end: f64,
) -> Vec<WordBoundary> {
    if words.len() < 2 || words.iter().filter(|w| w.end_sec > w.start_sec).count() < 2 {
        return Vec::new();
    }
    let is_timed = |i: usize| words[i].end_sec > words[i].start_sec;
    let is_punct_only = |i: usize| !words[i].w.chars().any(char::is_alphanumeric);
    let resolved = words.iter().any(|w| w.end_sec <= w.start_sec).then(|| {
        let texts: Vec<&str> = words.iter().map(|w| w.w.as_str()).collect();
        crate::transcribe::word_speakers::resolve_word_bounds(&texts, words, utt_start, utt_end)
    });

    let mut boundaries = Vec::with_capacity(words.len() - 1);
    for k in 0..words.len() - 1 {
        let ends = ends_sentence(&words[k].w);
        let dash = starts_turn_dash(&words[k + 1].w);
        let (cut_t, pause) = match resolved.as_deref() {
            Some(bounds) if !is_timed(k) || !is_timed(k + 1) => {
                let mid_l = f64::midpoint(bounds[k].0, bounds[k].1);
                let mid_r = f64::midpoint(bounds[k + 1].0, bounds[k + 1].1);
                let raw_cut = if !is_timed(k + 1) && is_punct_only(k + 1) && is_timed(k) {
                    f64::midpoint(bounds[k].1, mid_r)
                } else if !is_timed(k) && is_punct_only(k) && is_timed(k + 1) {
                    f64::midpoint(mid_l, bounds[k + 1].0)
                } else {
                    f64::midpoint(bounds[k].1, bounds[k + 1].0)
                };
                // Округление границ до 0.01 с не должно перекинуть «—» в левую половину.
                let cut_t = if mid_r - mid_l > 0.012 {
                    raw_cut.clamp(mid_l + 0.006, mid_r - 0.006)
                } else {
                    raw_cut.clamp(mid_l.min(mid_r), mid_l.max(mid_r))
                };
                let mut l = k;
                while l > 0 && !is_timed(l) && is_punct_only(l) {
                    l -= 1;
                }
                let mut r = k + 1;
                while r + 1 < words.len() && !is_timed(r) && is_punct_only(r) {
                    r += 1;
                }
                let pause = if is_timed(l) && is_timed(r) {
                    let owner = ((l + 1)..r)
                        .find(|&idx| starts_turn_dash(&words[idx].w))
                        .map_or(r - 1, |idx| idx - 1);
                    if k == owner {
                        (words[r].start_sec - words[l].end_sec).max(0.0)
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                (cut_t, pause)
            }
            _ => {
                let gap_start = words[k].end_sec;
                let gap_end = words[k + 1].start_sec;
                (
                    f64::midpoint(gap_start, gap_end),
                    (gap_end - gap_start).max(0.0),
                )
            }
        };
        boundaries.push(WordBoundary {
            cut_t,
            pause,
            ends_sentence: ends,
            starts_turn_dash: dash,
        });
    }
    boundaries
}

pub(crate) fn split_by_word_pauses(utterances: &[Utterance], min_gap_sec: f64) -> Vec<Utterance> {
    const MIN_UNIT_SEC: f64 = 1.0;
    let mut out = Vec::with_capacity(utterances.len());
    for u in utterances {
        if u.words.is_empty() || u.end - u.start < MIN_UNIT_SEC * 2.0 {
            out.push(u.clone());
            continue;
        }
        let boundaries = word_boundaries(&u.words, u.start, u.end);
        if boundaries.is_empty() {
            out.push(u.clone());
            continue;
        }
        let mut bounds = vec![u.start];
        for b in boundaries {
            // Быстрый обмен репликами с `.`/`!`/`?`/`…` перед «—» остаётся одним
            // чанком при паузе < 0.8 с, если резать только по длине паузы.
            let turn_dash = b.ends_sentence && b.starts_turn_dash;
            if b.pause >= min_gap_sec || turn_dash {
                let cut = (b.cut_t * 100.0).round() / 100.0;
                let from = *bounds.last().unwrap();
                if cut - from >= MIN_UNIT_SEC && u.end - cut >= MIN_UNIT_SEC {
                    bounds.push(cut);
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
pub(super) const SPLIT_MIN_DUR: f64 = 3.0;
const SPLIT_MAX_WINDOWS: usize = 8;
pub(super) const SPLIT_WINDOW_SEC: f64 = 1.6;

/// Шаг скользящего окна: на коротких репликах 0.5 с, на длинных растягивается
/// так, чтобы уложиться в [`SPLIT_MAX_WINDOWS`].
pub(super) fn split_step(duration: f64) -> f64 {
    let span = (duration - SPLIT_WINDOW_SEC).max(0.0);
    (span / (SPLIT_MAX_WINDOWS - 1) as f64).max(0.5)
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

    fn utterance(start: f64, end: f64, words: Vec<WordConf>) -> Utterance {
        let text = words
            .iter()
            .map(|w| w.w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Utterance {
            start,
            end,
            speaker: "X".to_string(),
            channel: Some(1),
            text,
            diarized: false,
            silhouette: 0.0,
            words,
            ctc_used: true,
        }
    }

    #[test]
    fn rapid_turn_dash_after_sentence_end_splits_below_min_gap() {
        for punct in [".", "!", "?", "…"] {
            let end_word = format!("слово2{punct}");
            let timed_dash = utterance(
                0.0,
                3.2,
                vec![
                    word("слово1", 0.2, 0.7),
                    word(&end_word, 0.8, 1.4),
                    word("—", 1.55, 1.60),
                    word("слово3", 1.70, 2.3),
                    word("слово4", 2.4, 3.0),
                ],
            );
            let out = split_by_word_pauses(&[timed_dash], 0.8);
            assert_eq!(out.len(), 2, "punct={punct}");
            assert_eq!(out[0].text, format!("слово1 {end_word}"));
            assert_eq!(out[1].text, "— слово3 слово4");
            assert!(out[0].end - out[0].start >= 1.0);
            assert!(out[1].end - out[1].start >= 1.0);
        }

        let untimed_punct = utterance(
            0.0,
            3.2,
            vec![
                word("слово1", 0.2, 0.7),
                word("слово2", 0.8, 1.3),
                word("...", 0.0, 0.0),
                word("—", 0.0, 0.0),
                word("слово3", 1.7, 2.4),
                word("слово4", 2.5, 3.0),
            ],
        );
        let out_untimed = split_by_word_pauses(&[untimed_punct], 0.8);
        assert_eq!(out_untimed.len(), 2);
        assert_eq!(out_untimed[0].text, "слово1 слово2 ...");
        assert_eq!(out_untimed[1].text, "— слово3 слово4");
        assert!(out_untimed[0].end - out_untimed[0].start >= 1.0);
        assert!(out_untimed[1].end - out_untimed[1].start >= 1.0);

        let prefixed = utterance(
            0.0,
            3.2,
            vec![
                word("слово1", 0.2, 0.7),
                word("слово2?", 0.8, 1.4),
                word("—слово3", 1.65, 2.3),
                word("слово4", 2.4, 3.0),
            ],
        );
        let out_prefixed = split_by_word_pauses(&[prefixed], 0.8);
        assert_eq!(out_prefixed.len(), 2);
        assert_eq!(out_prefixed[0].text, "слово1 слово2?");
        assert_eq!(out_prefixed[1].text, "—слово3 слово4");
        assert!(out_prefixed[0].end - out_prefixed[0].start >= 1.0);
        assert!(out_prefixed[1].end - out_prefixed[1].start >= 1.0);
    }

    #[test]
    fn turn_dash_respects_min_unit_and_ignores_mid_phrase_or_quoted_dash() {
        let short_left = utterance(
            0.0,
            3.0,
            vec![
                word("слово1.", 0.1, 0.5),
                word("—", 0.0, 0.0),
                word("слово2", 0.7, 1.6),
                word("слово3", 1.7, 2.8),
            ],
        );
        assert_eq!(split_by_word_pauses(&[short_left], 0.8).len(), 1);

        let mid_phrase_dash = utterance(
            0.0,
            3.2,
            vec![
                word("слово1", 0.2, 0.7),
                word("слово2", 0.8, 1.4),
                word("—", 0.0, 0.0),
                word("слово3", 1.6, 2.3),
                word("слово4", 2.4, 3.0),
            ],
        );
        assert_eq!(split_by_word_pauses(&[mid_phrase_dash], 0.8).len(), 1);

        let quoted_dash = utterance(
            0.0,
            3.2,
            vec![
                word("слово1", 0.2, 0.7),
                word("«слово2.»", 0.8, 1.4),
                word("—", 0.0, 0.0),
                word("слово3", 1.6, 2.3),
                word("слово4", 2.4, 3.0),
            ],
        );
        assert_eq!(split_by_word_pauses(&[quoted_dash], 0.8).len(), 1);
    }

    #[test]
    fn overlapping_timed_words_around_untimed_dash_do_not_panic() {
        let overlapping = utterance(
            0.0,
            3.2,
            vec![
                word("слово1", 0.2, 0.9),
                word("слово2.", 1.0, 1.55),
                word("—", 0.0, 0.0),
                word("слово3", 1.45, 1.55),
                word("слово4", 1.6, 3.0),
            ],
        );
        let boundaries = word_boundaries(&overlapping.words, overlapping.start, overlapping.end);
        assert_eq!(boundaries.len(), 4);
        assert!(boundaries
            .iter()
            .all(|b| b.cut_t.is_finite() && b.cut_t >= 0.0 && b.cut_t <= 3.2));
        let split = split_by_word_pauses(&[overlapping], 0.8);
        assert_eq!(split.len(), 2, "{split:?}");
        assert!(split.iter().all(|u| u.start < u.end), "{split:?}");

        let earlier_than_utt = utterance(
            1.0,
            4.2,
            vec![
                word("—", 0.0, 0.0),
                word("слово1.", 0.8, 1.2),
                word("—", 0.0, 0.0),
                word("слово2", 1.15, 2.5),
                word("слово3", 2.6, 4.0),
            ],
        );
        let early_bounds = word_boundaries(
            &earlier_than_utt.words,
            earlier_than_utt.start,
            earlier_than_utt.end,
        );
        assert!(early_bounds.iter().all(|b| b.cut_t.is_finite()));
        let early_split = split_by_word_pauses(&[earlier_than_utt], 0.8);
        assert!(!early_split.is_empty());
    }
}
