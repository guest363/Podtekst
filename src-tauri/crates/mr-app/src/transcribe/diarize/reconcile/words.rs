use std::collections::HashMap;

use crate::transcribe::Utterance;

use super::super::text::word_count;

/// Приклеить текст с учётом общего стыка: ASR мог распознать одни и те же
/// слова в перекрытии кусков. Срезаем самый длинный повторяющийся стык.
pub(super) fn overlap_word_count(a: &str, b: &str) -> usize {
    let aa: Vec<&str> = a.split_whitespace().collect();
    let bb: Vec<&str> = b.split_whitespace().collect();
    let norm = |w: &str| {
        w.trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    };
    let max_k = aa.len().min(bb.len()).min(10);
    for cand in (1..=max_k).rev() {
        let same = aa[aa.len() - cand..]
            .iter()
            .zip(&bb[..cand])
            .all(|(x, y)| norm(x) == norm(y));
        if same {
            return cand;
        }
    }
    0
}

pub(super) fn append_overlap_text(last: &mut String, next: &str) {
    let b: Vec<&str> = next.split_whitespace().collect();
    let k = overlap_word_count(last, next);
    if k < b.len() {
        last.push(' ');
        last.push_str(&b[k..].join(" "));
    }
}

pub(super) fn utterance_rms(
    samples: &HashMap<usize, Vec<f32>>,
    u: &Utterance,
    start: f64,
    end: f64,
) -> Option<f32> {
    let channel = u.channel? as usize;
    let data = samples.get(&channel)?;
    let sr = 16000.0;
    let lo = (start.max(0.0) * sr) as usize;
    let hi = (end.max(0.0) * sr) as usize;
    let hi = hi.min(data.len());
    if hi <= lo {
        return Some(0.0);
    }
    let sum = data[lo..hi]
        .iter()
        .map(|&x| (x as f64) * (x as f64))
        .sum::<f64>();
    Some((sum / (hi - lo) as f64).sqrt() as f32)
}

pub(super) fn better_utterance(a: &Utterance, b: &Utterance) -> bool {
    let wa = word_count(&a.text);
    let wb = word_count(&b.text);
    // Системный канал обычно чище микрофона (там нет эха комнаты). Берём
    // его, если он не потерял больше ~30% слов; иначе доверяем более полному.
    if a.channel != b.channel {
        if a.channel == Some(1) && wa * 10 >= wb * 7 {
            return true;
        }
        if b.channel == Some(1) && wb * 10 >= wa * 7 {
            return false;
        }
    }
    if wa != wb {
        return wa > wb;
    }
    a.end - a.start >= b.end - b.start
}
