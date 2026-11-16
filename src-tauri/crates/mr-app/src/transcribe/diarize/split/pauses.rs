use crate::transcribe::Utterance;

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
pub(super) const SPLIT_MIN_DUR: f64 = 3.0;
const SPLIT_MAX_WINDOWS: usize = 8;
pub(super) const SPLIT_WINDOW_SEC: f64 = 1.6;

/// Шаг скользящего окна: на коротких репликах 0.5 с, на длинных растягивается
/// так, чтобы уложиться в [`SPLIT_MAX_WINDOWS`].
pub(super) fn split_step(duration: f64) -> f64 {
    let span = (duration - SPLIT_WINDOW_SEC).max(0.0);
    (span / (SPLIT_MAX_WINDOWS - 1) as f64).max(0.5)
}
