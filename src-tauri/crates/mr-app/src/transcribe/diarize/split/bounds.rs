use crate::transcribe::{Utterance, WordConf};

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
