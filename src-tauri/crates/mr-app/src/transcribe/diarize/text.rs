use std::collections::HashSet;

use crate::transcribe::Utterance;

/// ASR-сегмент с padding иногда дослышит начало следующей реплики: слова
/// получают время уже за `end` этого сегмента. Переносим такой хвост в
/// соседнюю реплику, иначе он наследует чужого спикера.
pub(crate) fn move_boundary_words(utterances: &mut Vec<Utterance>) {
    if utterances.len() < 2 {
        return;
    }
    utterances.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut i = 0;
    while i + 1 < utterances.len() {
        if utterances[i].channel != utterances[i + 1].channel
            || utterances[i + 1].start - utterances[i].end > 0.5
        {
            i += 1;
            continue;
        }
        let split = utterances[i]
            .words
            .iter()
            .position(|w| w.start_sec >= utterances[i].end - 0.05)
            .unwrap_or(utterances[i].words.len());
        if split == utterances[i].words.len() || split == 0 {
            i += 1;
            continue;
        }
        let moved = utterances[i].words.split_off(split);
        utterances[i].text = utterances[i]
            .words
            .iter()
            .map(|w| w.w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let moved_text = moved
            .iter()
            .map(|w| w.w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let next = &mut utterances[i + 1];
        if let Some(first) = moved.first() {
            next.start = next.start.min(first.start_sec);
        }
        next.end = next.end.max(moved.last().map_or(0.0, |w| w.end_sec));
        next.words.splice(0..0, moved);
        next.text = if next.text.is_empty() {
            moved_text
        } else {
            format!("{moved_text} {}", next.text)
        };
        if utterances[i].text.is_empty() {
            utterances.remove(i);
        } else {
            i += 1;
        }
    }
}

pub(crate) fn normalize_word(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
        .chars()
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

fn words_of(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(normalize_word)
        .collect()
}

pub(super) fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

pub(super) fn words_contained(small: &str, big: &str) -> bool {
    let a = words_of(small);
    let b = words_of(big);
    !a.is_empty() && a.is_subset(&b)
}

pub(crate) fn words_match(a: &str, b: &str) -> bool {
    if a == b || (a == "че" && b == "что") || (a == "что" && b == "че") {
        return true;
    }
    let ca: Vec<char> = a.chars().collect();
    let cb: Vec<char> = b.chars().collect();
    if ca.len() >= 4 && cb.len() >= 4 && ca.len().abs_diff(cb.len()) <= 3 {
        if ca.len() == 4 && cb.len() == 4 {
            // Замер эхо-дедупликации: расхождение словоформ 4-символьных лексем
            // лежит в флексии на гласной; согласный хвост различает лексемы.
            ca[..3] == cb[..3] && inflection_tail(ca[3]) && inflection_tail(cb[3])
        } else {
            ca[..4] == cb[..4]
        }
    } else {
        false
    }
}

fn inflection_tail(c: char) -> bool {
    matches!(
        c,
        'а' | 'е' | 'и' | 'о' | 'у' | 'ы' | 'э' | 'ю' | 'я' | 'a' | 'e' | 'i' | 'o' | 'u' | 'y'
    )
}

/// Похожесть двух распознанных фраз (Jaccard по словам). Эхо одной и той же
/// речи в двух каналах обычно даёт 0.6+, разные фразы — заметно меньше.
pub(crate) fn text_similarity(a: &str, b: &str) -> f64 {
    let wa = words_of(a);
    let wb = words_of(b);
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    let inter = wa.intersection(&wb).count() as f64;
    let union = wa.union(&wb).count() as f64;
    let j = inter / union.max(1.0);
    // shortcut: порог 0.65 включения слов для реплик >= 2 слов подавляет эхо
    // при небольших расхождениях ASR (например, искажение 1-2 слов в акустическом тракте).
    let (small, big) = if wa.len() <= wb.len() {
        (&wa, &wb)
    } else {
        (&wb, &wa)
    };
    let matched = small
        .iter()
        .filter(|sw| big.iter().any(|bw| words_match(sw, bw)))
        .count();
    let containment = matched as f64 / small.len() as f64;
    // Для одиночного слова Jaccard по словоформам даёт 0: эхо-дубль однословной
    // реплики сводим к words_match. Вхождение слова в длинную фразу чужого
    // канала эхом не считаем — это бэкканал.
    if containment >= 0.65 && (small.len() >= 2 || big.len() == 1) {
        j.max(0.85)
    } else {
        j
    }
}

pub(super) fn interval_overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}
