use std::collections::HashMap;

use crate::transcribe::word_speakers::resolve_word_bounds;
use crate::transcribe::{Utterance, WordConf};

use super::bounds::resolved_word_conf;
use super::pauses::{ends_sentence, starts_turn_dash};

/// Минимальная длительность реплики `ch=1` для доразбиения по чистым прогонам
/// `PyAnnote`: короткие фрагменты уже покрываются разрезанием по паузам.
const REFINE_MIN_UTTERANCE_SEC: f64 = 6.0;
/// Чистый прогон и оба отступа от краёв реплики не короче 1 с, иначе
/// случайные всплески или края дают рваные обрывки.
const REFINE_MIN_RUN_SEC: f64 = 1.0;
const REFINE_MIN_PAUSE_SEC: f64 = 0.12;
/// Допуск между краем прогона `PyAnnote` и границей слова: учитывает окно
/// сглаживания сегментации и отброшенные кадры перекрытия перед речью.
const REFINE_ALIGN_TOLERANCE_SEC: f64 = 0.5;
/// Склейка соседних односпикерных прогонов через короткое перекрытие/тишину,
/// чтобы один внутренний стык не дробил непрерывный ответ собеседника.
const REFINE_RUN_COALESCE_GAP_SEC: f64 = 0.5;

fn is_lexical_boundary(prev: &str, next: &str) -> bool {
    let trimmed = prev.trim_end_matches(['"', '\'', '»', '”', ')', ']']);
    ends_sentence(trimmed) || (trimmed != "—" && trimmed.ends_with('—')) || starts_turn_dash(next)
}

fn coalesce_clean_runs(voice_timeline: &[Utterance]) -> Vec<Utterance> {
    let mut clean: Vec<Utterance> = voice_timeline
        .iter()
        .filter(|run| run.end - run.start >= REFINE_MIN_RUN_SEC && !run.speaker.is_empty())
        .cloned()
        .collect();
    clean.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
    let mut merged: Vec<Utterance> = Vec::with_capacity(clean.len());
    for run in clean {
        if let Some(last) = merged.last_mut() {
            if last.speaker == run.speaker && run.start - last.end < REFINE_RUN_COALESCE_GAP_SEC {
                last.end = last.end.max(run.end);
                continue;
            }
        }
        merged.push(run);
    }
    merged
}

struct WordBoundary {
    index: usize,
    lo: f64,
    hi: f64,
    mid: f64,
}

fn eligible_boundaries(
    utterance: &Utterance,
    words: &[&str],
    word_bounds: &[(f64, f64)],
    has_timed_words: bool,
) -> Vec<WordBoundary> {
    let mut out = Vec::new();
    for i in 0..words.len().saturating_sub(1) {
        if words[i + 1..].iter().all(|&word| word == "—") {
            continue;
        }
        let prev_end = word_bounds[i].1;
        // Тире без собственных таймкодов в CTC растягивается на всю паузу между словами,
        // поэтому реальное окно тишины перед репликой доходит до следующего слова.
        let next_start = if words[i + 1] == "—"
            && utterance
                .words
                .get(i + 1)
                .is_none_or(|word| word.end_sec <= word.start_sec)
            && i + 2 < words.len()
        {
            word_bounds[i + 2].0
        } else {
            word_bounds[i + 1].0
        };
        let is_pause = has_timed_words && (next_start - prev_end) >= REFINE_MIN_PAUSE_SEC;
        let is_lexical = is_lexical_boundary(words[i], words[i + 1]);
        if !is_pause && !is_lexical {
            continue;
        }
        out.push(WordBoundary {
            index: i,
            lo: prev_end.min(next_start),
            hi: prev_end.max(next_start),
            mid: f64::midpoint(prev_end, next_start),
        });
    }
    out
}

fn align_edge_to_boundary(
    edge: f64,
    utterance_start: f64,
    utterance_end: f64,
    has_timed_words: bool,
    boundaries: &[WordBoundary],
) -> Option<(usize, f64)> {
    let interval_dist = |bound: &WordBoundary| {
        if edge < bound.lo {
            bound.lo - edge
        } else if edge > bound.hi {
            edge - bound.hi
        } else {
            0.0
        }
    };
    boundaries
        .iter()
        .filter_map(|bound| {
            let dist = interval_dist(bound);
            if dist > REFINE_ALIGN_TOLERANCE_SEC {
                return None;
            }
            let cut_t = if has_timed_words { bound.mid } else { edge };
            if cut_t < utterance_start + REFINE_MIN_RUN_SEC
                || cut_t > utterance_end - REFINE_MIN_RUN_SEC
            {
                return None;
            }
            Some((bound.index, cut_t, dist, (edge - bound.mid).abs()))
        })
        .min_by(|a, b| {
            a.2.total_cmp(&b.2)
                .then_with(|| a.3.total_cmp(&b.3))
                .then_with(|| a.0.cmp(&b.0))
        })
        .map(|(index, cut_t, _, _)| (index, cut_t))
}

fn dominant_piece_speaker(
    utterance: &Utterance,
    runs: &[Utterance],
    start: f64,
    end: f64,
) -> String {
    let mut own_ov = 0.0f64;
    let mut others: HashMap<&str, f64> = HashMap::new();
    for run in runs {
        let ov = (end.min(run.end) - start.max(run.start)).max(0.0);
        if ov <= 0.0 {
            continue;
        }
        if run.speaker == utterance.speaker {
            own_ov += ov;
        } else {
            *others.entry(run.speaker.as_str()).or_default() += ov;
        }
    }
    let best_other = others
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)));
    match best_other {
        Some((speaker, ov))
            if ov >= REFINE_MIN_RUN_SEC && ov > own_ov && ov >= (end - start) * 0.5 =>
        {
            speaker.to_string()
        }
        _ => utterance.speaker.clone(),
    }
}

fn collect_voice_run_cuts(
    utterance: &Utterance,
    runs: &[Utterance],
    has_timed_words: bool,
    boundaries: &[WordBoundary],
) -> Vec<(usize, f64)> {
    let align = |edge: f64| {
        align_edge_to_boundary(
            edge,
            utterance.start,
            utterance.end,
            has_timed_words,
            boundaries,
        )
    };
    let mut cuts: Vec<(usize, f64)> = Vec::new();
    for run in runs {
        if run.speaker == utterance.speaker {
            continue;
        }
        let ov_start = run.start.max(utterance.start);
        let ov_end = run.end.min(utterance.end);
        if ov_end - ov_start < REFINE_MIN_RUN_SEC {
            continue;
        }
        let start_interior = run.start >= utterance.start + REFINE_MIN_RUN_SEC
            && run.start <= utterance.end - REFINE_MIN_RUN_SEC;
        let end_interior = run.end >= utterance.start + REFINE_MIN_RUN_SEC
            && run.end <= utterance.end - REFINE_MIN_RUN_SEC;
        match (start_interior, end_interior) {
            (true, true) => {
                if let (Some(a), Some(b)) = (align(run.start), align(run.end)) {
                    if b.0 > a.0 && b.1 - a.1 >= REFINE_MIN_RUN_SEC {
                        cuts.push(a);
                        cuts.push(b);
                    }
                }
            }
            (true, false) => cuts.extend(align(run.start)),
            (false, true) => cuts.extend(align(run.end)),
            (false, false) => {}
        }
    }
    cuts.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    let mut spaced: Vec<(usize, f64)> = Vec::new();
    for (word_idx, cut_t) in cuts {
        if spaced.last().is_none_or(|&(prev_idx, prev_t)| {
            word_idx > prev_idx && cut_t - prev_t >= REFINE_MIN_RUN_SEC
        }) {
            spaced.push((word_idx, cut_t));
        }
    }
    spaced
}

fn slice_part_words(
    utterance: &Utterance,
    words: &[&str],
    word_bounds: &[(f64, f64)],
    has_timed_words: bool,
    word_range: (usize, usize),
    time_bounds: (f64, f64),
) -> Vec<WordConf> {
    let (lo, hi) = word_range;
    if !has_timed_words {
        return utterance.words.get(lo..hi).unwrap_or(&[]).to_vec();
    }
    (lo..hi)
        .map(|idx| {
            resolved_word_conf(
                utterance.words.get(idx),
                words[idx],
                word_bounds[idx],
                time_bounds,
            )
        })
        .collect()
}

fn split_utterance_by_voice_runs(utterance: &Utterance, runs: &[Utterance]) -> Vec<Utterance> {
    if utterance.end - utterance.start < REFINE_MIN_UTTERANCE_SEC {
        return vec![utterance.clone()];
    }
    let words: Vec<&str> = utterance.text.split_whitespace().collect();
    if words.len() < 2 {
        return vec![utterance.clone()];
    }
    let has_timed_words = utterance.words.iter().any(|w| w.end_sec > w.start_sec);
    let word_bounds = resolve_word_bounds(&words, &utterance.words, utterance.start, utterance.end);
    let boundaries = eligible_boundaries(utterance, &words, &word_bounds, has_timed_words);
    let spaced = if boundaries.is_empty() {
        Vec::new()
    } else {
        collect_voice_run_cuts(utterance, runs, has_timed_words, &boundaries)
    };
    if spaced.is_empty() {
        return vec![utterance.clone()];
    }
    let mut parts: Vec<Utterance> = Vec::with_capacity(spaced.len() + 1);
    let mut word_lo = 0usize;
    let mut time_lo = utterance.start;
    for k in 0..=spaced.len() {
        let (word_hi, time_hi) = if k < spaced.len() {
            (spaced[k].0 + 1, (spaced[k].1 * 100.0).round() / 100.0)
        } else {
            (words.len(), utterance.end)
        };
        let part_words = slice_part_words(
            utterance,
            &words,
            &word_bounds,
            has_timed_words,
            (word_lo, word_hi),
            (time_lo, time_hi),
        );
        let speaker = dominant_piece_speaker(utterance, runs, time_lo, time_hi);
        let fragment_text = words[word_lo..word_hi].join(" ");
        if let Some(last) = parts.last_mut().filter(|last| last.speaker == speaker) {
            last.end = time_hi;
            if !last.text.is_empty() {
                last.text.push(' ');
            }
            last.text.push_str(&fragment_text);
            last.words.extend(part_words);
        } else {
            parts.push(Utterance {
                start: time_lo,
                end: time_hi,
                speaker,
                channel: utterance.channel,
                text: fragment_text,
                diarized: utterance.diarized,
                silhouette: utterance.silhouette,
                words: part_words,
                ctc_used: utterance.ctc_used,
            });
        }
        word_lo = word_hi;
        time_lo = time_hi;
    }
    if parts.len() <= 1 {
        vec![utterance.clone()]
    } else {
        parts
    }
}

pub(in crate::transcribe::diarize) fn refine_ch1_utterances(
    utterances: &mut Vec<Utterance>,
    voice_timeline: &[Utterance],
    protected: &HashMap<usize, usize>,
    promoted_utts: &HashMap<usize, usize>,
) {
    let runs = coalesce_clean_runs(voice_timeline);
    if runs.is_empty()
        || !utterances
            .iter()
            .any(|u| u.end - u.start >= REFINE_MIN_UTTERANCE_SEC)
    {
        return;
    }
    let mut refined = Vec::with_capacity(utterances.len());
    for (index, utterance) in utterances.iter().enumerate() {
        // Защищённые и одиночные реплики неизвестных голосов могут иметь иной
        // ярлык в voice_timeline: дробление вернуло бы реплику соседу.
        if utterance.end - utterance.start >= REFINE_MIN_UTTERANCE_SEC
            && !protected.contains_key(&index)
            && !promoted_utts.contains_key(&index)
        {
            refined.extend(split_utterance_by_voice_runs(utterance, &runs));
        } else {
            refined.push(utterance.clone());
        }
    }
    *utterances = refined;
}
