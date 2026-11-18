use std::collections::HashMap;

use super::diarize;
use super::echo::{self, EchoEvidence};
use super::word_speakers::DiarIntervalTree;
use super::{Utterance, WordConf};

const SAMPLE_RATE: f64 = 16_000.0;
const LOCAL_ONLY_SHARE: f32 = 0.5;
const DOUBLE_TALK_SHARE: f32 = 0.3;
/// Калибровка на живых записях: остаток ниже 15% уровня микрофона — уже не
/// локальная речь, а неточность линейного предсказания эха (см. `echo::RESIDUAL_RELATIVE`).
const RESIDUAL_KEEP: f32 = 0.15;
/// Двойная речь даёт остаток уже с 10% из-за наложения двух источников.
const DOUBLE_TALK_RESIDUAL: f32 = 0.10;
const ECHO_SHARE: f32 = 0.7;
/// Приглушённое эхо комнаты может превысить медианный `gain` по файлу до 2.3x,
/// когда на локальном отрезке почти нет чистых локальных кадров (`local_only <= 0.05`).
const MAX_MUFFLED_LOCAL_ONLY: f32 = 0.05;
const MUFFLED_SYS_NOISE_MULT: f32 = 5.0;
const MUFFLED_GAIN_MULT: f32 = 2.3;
/// Дрейф оценки задержки между зондами: ±60 мс перекрывают округление до кадра
/// (32 мс) и медленное расхождение часов каналов.
const DELAY_DRIFT_SEC: f64 = 0.06;
/// Остаток эха короче трёх слов — ошибки ASR, а не реплика; короткие ответы не трогаем.
const MIN_LOCAL_WORDS: usize = 3;
/// Доля локальных слов, ниже которой остаток реплики считается шумом: правило
/// по доле, а не по абсолютному числу, иначе длинная реплика теряет реальные слова.
const MIN_LOCAL_SHARE: f32 = 0.25;

/// Пословная дедупликация эха в микрофонном канале; счётчик изменённых
/// реплик нужен только для лога `ev=diar.word_dedupe`.
pub(crate) fn dedupe_words(
    utterances: &mut Vec<Utterance>,
    samples: &HashMap<usize, Vec<f32>>,
    echo: &EchoEvidence,
    tree: Option<&DiarIntervalTree>,
) -> usize {
    let Some(mic) = samples.get(&0) else {
        return 0;
    };
    let sys = samples.get(&1).map(Vec::as_slice);
    let other_spans: Vec<(f64, f64)> = utterances
        .iter()
        .filter(|u| u.channel == Some(1))
        .map(|u| (u.start, u.end))
        .collect();
    let mut echo_words: Vec<(f64, f64, String)> = utterances
        .iter()
        .filter(|u| u.channel == Some(1))
        .flat_map(|u| u.words.iter())
        .filter(|w| w.end_sec > w.start_sec)
        .map(|w| (w.start_sec, w.end_sec, diarize::normalize_word(&w.w)))
        .filter(|(_, _, word)| !word.is_empty())
        .collect();
    if other_spans.is_empty() || echo_words.is_empty() {
        return 0;
    }
    echo_words.sort_by(|a, b| a.0.total_cmp(&b.0));
    let max_span = echo_words
        .iter()
        .map(|(start, end, _)| (end - start).max(0.0))
        .fold(0.0, f64::max);

    let mut changed = 0usize;
    let mut dropped = 0usize;
    let mut dropped_words = 0usize;
    let mut kept: Vec<Utterance> = Vec::with_capacity(utterances.len());
    for mut utterance in utterances.drain(..) {
        if utterance.channel != Some(0)
            || !utterance.words.iter().any(|w| w.end_sec > w.start_sec)
            || !overlaps_any(&utterance, &other_spans)
        {
            kept.push(utterance);
            continue;
        }
        let local = local_words(&utterance, mic, sys, echo, &echo_words, max_span);
        let real_local_words = count_real_words(&local);
        // Токены голой пунктуации вроде «— — —» не содержат речи: без слов
        // после отсева эха такую галлюцинацию нельзя оставлять в микрофоне.
        if real_local_words == 0 {
            changed += 1;
            dropped += 1;
            dropped_words += utterance.words.len();
            continue;
        }
        if local.len() == utterance.words.len() {
            kept.push(utterance);
            continue;
        }
        changed += 1;
        let real_total_words = count_real_words(&utterance.words);
        let local_share = real_local_words as f32 / real_total_words.max(1) as f32;
        if real_local_words < MIN_LOCAL_WORDS && local_share < MIN_LOCAL_SHARE {
            dropped += 1;
            dropped_words += utterance.words.len() - local.len();
            continue;
        }
        let Some(first) = local.first() else {
            continue;
        };
        let last = local.last().unwrap_or(first);
        let start = first.start_sec;
        let end = last.end_sec;
        let text = local
            .iter()
            .map(|w| w.w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        utterance.text = text;
        utterance.start = start;
        utterance.end = end;
        if let Some(tree) = tree {
            if let Some((speaker, _)) = tree.assign_dominant_speaker(start, end) {
                if !speaker.is_empty() && speaker != utterance.speaker {
                    utterance.speaker = speaker;
                }
            }
        }
        utterance.words = local;
        kept.push(utterance);
    }
    *utterances = kept;
    if dropped > 0 {
        log::debug!(
            target: "diar",
            utterances = dropped,
            words = dropped_words;
            "ev=word_dedupe.drop"
        );
    }
    changed
}

fn count_real_words(words: &[WordConf]) -> usize {
    words
        .iter()
        .filter(|word| !diarize::normalize_word(&word.w).is_empty())
        .count()
}

fn overlaps_any(utterance: &Utterance, spans: &[(f64, f64)]) -> bool {
    spans
        .iter()
        .any(|&(start, end)| utterance.end.min(end) - utterance.start.max(start) > 0.0)
}

fn local_words(
    utterance: &Utterance,
    mic: &[f32],
    sys: Option<&[f32]>,
    echo: &EchoEvidence,
    echo_words: &[(f64, f64, String)],
    max_span: f64,
) -> Vec<WordConf> {
    utterance
        .words
        .iter()
        .filter(|word| !is_echo(word, mic, sys, echo, echo_words, max_span))
        .cloned()
        .collect()
}

fn is_echo(
    word: &WordConf,
    mic: &[f32],
    sys: Option<&[f32]>,
    echo: &EchoEvidence,
    echo_words: &[(f64, f64, String)],
    max_span: f64,
) -> bool {
    if word.end_sec <= word.start_sec {
        return false;
    }
    let normalized = diarize::normalize_word(&word.w);
    if normalized.is_empty() {
        return false;
    }
    let profile = echo::segment_profile(echo, word.start_sec, word.end_sec);
    if profile.local_only > LOCAL_ONLY_SHARE {
        return false;
    }
    // Усиление эха — медиана по файлу: при смене громкости динамиков остаток
    // выглядит как двойная речь, поэтому совпадение текста в окне задержки решает.
    if matches_echo_word(
        &normalized,
        word.start_sec,
        word.end_sec,
        echo.delay_sec,
        echo_words,
        max_span,
    ) {
        return true;
    }
    let rms = channel_rms(mic, word.start_sec, word.end_sec);
    let residual_ratio = echo::mean_residual(echo, word.start_sec, word.end_sec) / rms.max(1e-6);
    if profile.double_talk > DOUBLE_TALK_SHARE && residual_ratio > DOUBLE_TALK_RESIDUAL {
        return false;
    }
    if profile.echo_only + profile.uncertain < ECHO_SHARE {
        return false;
    }
    if residual_ratio <= RESIDUAL_KEEP {
        return true;
    }
    if echo.is_strong() && profile.local_only <= MAX_MUFFLED_LOCAL_ONLY {
        if let Some(sys_samples) = sys {
            let sys_rms = channel_rms(
                sys_samples,
                word.start_sec - echo.delay_sec,
                word.end_sec - echo.delay_sec,
            );
            if sys_rms > echo.noise_floor * MUFFLED_SYS_NOISE_MULT
                && rms <= echo.gain * sys_rms * MUFFLED_GAIN_MULT
            {
                return true;
            }
        }
    }
    false
}

fn matches_echo_word(
    word: &str,
    start: f64,
    end: f64,
    delay_sec: f64,
    echo_words: &[(f64, f64, String)],
    max_span: f64,
) -> bool {
    [
        delay_sec - DELAY_DRIFT_SEC,
        delay_sec,
        delay_sec + DELAY_DRIFT_SEC,
    ]
    .iter()
    .any(|shift| {
        // Кандидат обязан начинаться не позже `end - shift` и перекрывать
        // `start`; нижняя граница отсекает слова, которые не дотянутся.
        let lo = echo_words.partition_point(|w| w.0 < start - shift - max_span);
        let hi = echo_words.partition_point(|w| w.0 < end - shift);
        if lo >= hi {
            return false;
        }
        echo_words[lo..hi]
            .iter()
            .any(|(other_start, other_end, other)| {
                let shifted_start = other_start + shift;
                let shifted_end = other_end + shift;
                shifted_end > start && shifted_start < end && diarize::words_match(word, other)
            })
    })
}

fn channel_rms(samples: &[f32], start: f64, end: f64) -> f32 {
    let lo = (start.max(0.0) * SAMPLE_RATE) as usize;
    let hi = ((end.max(0.0) * SAMPLE_RATE) as usize).min(samples.len());
    if hi <= lo {
        return 0.0;
    }
    let sum = samples[lo..hi]
        .iter()
        .map(|&x| (x as f64) * (x as f64))
        .sum::<f64>();
    (sum / (hi - lo) as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::echo::FrameClass;

    const WIN: f64 = 512.0 / 16_000.0;

    fn evidence(spans: &[(f64, f64, FrameClass, f32)], total_sec: f64) -> EchoEvidence {
        let n = (total_sec / WIN).ceil() as usize;
        let mut frames = vec![FrameClass::Uncertain; n];
        let mut residual = vec![0.0f32; n];
        for (index, (frame, value)) in frames.iter_mut().zip(residual.iter_mut()).enumerate() {
            let t = index as f64 * WIN;
            if let Some((_, _, class, rest)) = spans.iter().find(|(a, b, _, _)| t >= *a && t < *b) {
                *frame = *class;
                *value = *rest;
            }
        }
        EchoEvidence {
            delay_samples: 0,
            delay_sec: 0.0,
            prominence: 12.0,
            stability: 1.0,
            gain: 0.1,
            noise_floor: 1e-4,
            frames,
            residual,
        }
    }

    fn word(text: &str, start: f64, end: f64) -> WordConf {
        WordConf {
            w: text.to_string(),
            conf: 0.9,
            needs_review: false,
            start_sec: start,
            end_sec: end,
        }
    }

    fn utterance(start: f64, end: f64, channel: i32, words: Vec<WordConf>) -> Utterance {
        Utterance {
            start,
            end,
            speaker: "Спикер".to_string(),
            channel: Some(channel),
            text: words
                .iter()
                .map(|w| w.w.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            diarized: false,
            silhouette: 0.0,
            words,
            ctc_used: false,
        }
    }

    fn samples() -> HashMap<usize, Vec<f32>> {
        HashMap::from([(0usize, vec![0.2f32; 16_000 * 3])])
    }

    fn mixed_evidence() -> EchoEvidence {
        evidence(
            &[
                (0.0, 0.55, FrameClass::EchoOnly, 0.0),
                (0.55, 1.25, FrameClass::LocalOnly, 0.0),
                (1.25, 2.0, FrameClass::EchoOnly, 0.0),
            ],
            2.0,
        )
    }

    fn mixed_utterances() -> Vec<Utterance> {
        vec![
            utterance(
                0.0,
                2.0,
                0,
                vec![
                    word("Альфа", 0.0, 0.5),
                    word("Бета", 0.6, 1.2),
                    word("Гамма", 1.3, 1.9),
                ],
            ),
            utterance(
                0.0,
                2.0,
                1,
                vec![word("альфа", 0.0, 0.5), word("гамма", 1.3, 1.9)],
            ),
        ]
    }

    #[test]
    fn echo_words_are_removed_and_local_word_kept() {
        let mut utts = mixed_utterances();
        let changed = dedupe_words(&mut utts, &samples(), &mixed_evidence(), None);
        assert_eq!(changed, 1, "{utts:?}");
        assert_eq!(utts.len(), 2, "{utts:?}");
        let mic = &utts[0];
        assert_eq!(mic.channel, Some(0));
        assert_eq!(mic.text, "Бета");
        assert!((mic.start - 0.6).abs() < 1e-9 && (mic.end - 1.2).abs() < 1e-9);
        assert_eq!(mic.words.len(), 1, "{mic:?}");
    }

    #[test]
    fn fully_echoed_utterance_is_dropped() {
        let mut utts = vec![
            utterance(
                0.0,
                2.0,
                0,
                vec![word("альфа", 0.0, 0.5), word("гамма", 1.3, 1.9)],
            ),
            utterance(
                0.0,
                2.0,
                1,
                vec![word("альфа", 0.0, 0.5), word("гамма", 1.3, 1.9)],
            ),
        ];
        let changed = dedupe_words(&mut utts, &samples(), &mixed_evidence(), None);
        assert_eq!(changed, 1);
        assert_eq!(utts.len(), 1, "{utts:?}");
        assert_eq!(utts[0].channel, Some(1));
    }

    #[test]
    fn long_utterance_with_two_local_words_keeps_them() {
        // 6 слов: 4 эха + 2 реальных локальных. Старое правило по абсолютному
        // числу выбрасывало всю реплику вместе с локальными словами.
        let ev = evidence(
            &[
                (0.0, 0.6, FrameClass::EchoOnly, 0.0),
                (0.6, 1.4, FrameClass::LocalOnly, 0.0),
                (1.4, 2.0, FrameClass::EchoOnly, 0.0),
            ],
            2.0,
        );
        let mut utts = vec![
            utterance(
                0.0,
                2.0,
                0,
                vec![
                    word("альфа", 0.0, 0.3),
                    word("альфа", 0.35, 0.55),
                    word("Бета", 0.65, 0.85),
                    word("Дельта", 0.9, 1.1),
                    word("гамма", 1.45, 1.7),
                    word("гамма", 1.75, 1.95),
                ],
            ),
            utterance(
                0.0,
                2.0,
                1,
                vec![
                    word("альфа", 0.0, 0.3),
                    word("альфа", 0.35, 0.55),
                    word("гамма", 1.45, 1.7),
                    word("гамма", 1.75, 1.95),
                ],
            ),
        ];
        let changed = dedupe_words(&mut utts, &samples(), &ev, None);
        assert_eq!(changed, 1, "{utts:?}");
        assert_eq!(utts.len(), 2, "реплика не должна исчезнуть: {utts:?}");
        assert_eq!(utts[0].text, "Бета Дельта");
    }

    #[test]
    fn double_talk_with_residual_keeps_local_word() {
        let ev = evidence(&[(0.0, 2.0, FrameClass::DoubleTalk, 0.024)], 2.0);
        let mut utts = vec![
            utterance(0.0, 1.0, 0, vec![word("дельта", 0.0, 0.5)]),
            utterance(0.0, 1.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let changed = dedupe_words(&mut utts, &samples(), &ev, None);
        assert_eq!(changed, 0, "{utts:?}");
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[0].text, "дельта");
    }

    #[test]
    fn matching_word_in_double_talk_is_echo() {
        let ev = evidence(&[(0.0, 2.0, FrameClass::DoubleTalk, 0.024)], 2.0);
        let mut utts = vec![
            utterance(0.0, 1.0, 0, vec![word("альфа", 0.0, 0.5)]),
            utterance(0.0, 1.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let changed = dedupe_words(&mut utts, &samples(), &ev, None);
        assert_eq!(changed, 1, "{utts:?}");
        assert_eq!(utts.len(), 1, "{utts:?}");
        assert_eq!(utts[0].channel, Some(1));
    }

    #[test]
    fn short_echo_residue_of_long_utterance_is_dropped() {
        let ev = evidence(&[(0.0, 2.0, FrameClass::EchoOnly, 0.0)], 2.0);
        let spans = ["альфа", "гамма", "дельта", "эпсилон"];
        let mut mic: Vec<WordConf> = spans
            .iter()
            .enumerate()
            .map(|(i, w)| word(w, i as f64 * 0.4, i as f64 * 0.4 + 0.35))
            .collect();
        mic.push(word("лишнее", 1.7, 1.9));
        let system: Vec<WordConf> = spans
            .iter()
            .enumerate()
            .map(|(i, w)| word(w, i as f64 * 0.4, i as f64 * 0.4 + 0.35))
            .collect();
        let mut utts = vec![utterance(0.0, 2.0, 0, mic), utterance(0.0, 2.0, 1, system)];
        let changed = dedupe_words(&mut utts, &samples(), &ev, None);
        assert_eq!(changed, 1, "{utts:?}");
        assert_eq!(utts.len(), 1, "{utts:?}");
        assert_eq!(utts[0].channel, Some(1));
    }

    #[test]
    fn utterance_without_word_timing_is_untouched() {
        let mut untimed = utterance(0.0, 2.0, 0, vec![word("альфа", 0.0, 0.0)]);
        untimed.text = "альфа".to_string();
        let mut utts = vec![
            untimed,
            utterance(0.0, 2.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let changed = dedupe_words(&mut utts, &samples(), &mixed_evidence(), None);
        assert_eq!(changed, 0, "{utts:?}");
        assert_eq!(utts[0].words.len(), 1);
        assert_eq!(utts[0].text, "альфа");
    }

    #[test]
    fn system_channel_words_are_untouched() {
        let mut utts = mixed_utterances();
        let changed = dedupe_words(&mut utts, &samples(), &mixed_evidence(), None);
        assert_eq!(changed, 1);
        let system = utts
            .iter()
            .find(|u| u.channel == Some(1))
            .expect("ch1 на месте");
        assert_eq!(system.words.len(), 2, "{system:?}");
        assert_eq!(system.text, "альфа гамма");
    }

    #[test]
    fn missing_mic_samples_change_nothing() {
        let mut utts = mixed_utterances();
        let empty = HashMap::new();
        let changed = dedupe_words(&mut utts, &empty, &mixed_evidence(), None);
        assert_eq!(changed, 0);
        assert_eq!(utts[0].words.len(), 3);
    }

    #[test]
    fn pure_punctuation_echo_utterance_is_dropped() {
        let ev = evidence(&[(0.0, 2.0, FrameClass::EchoOnly, 0.0)], 2.0);
        let mut utts = vec![
            utterance(
                0.0,
                2.0,
                0,
                vec![
                    word("—", 0.1, 0.3),
                    word("—", 0.4, 0.6),
                    word("—", 0.7, 0.9),
                ],
            ),
            utterance(0.0, 2.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let changed = dedupe_words(&mut utts, &samples(), &ev, None);
        assert_eq!(changed, 1, "{utts:?}");
        assert_eq!(utts.len(), 1, "{utts:?}");
        assert_eq!(utts[0].channel, Some(1));

        let mut mixed_punct = vec![
            utterance(
                0.0,
                2.0,
                0,
                vec![
                    word("альфа", 0.0, 0.3),
                    word("альфа", 0.35, 0.55),
                    word("альфа", 0.6, 0.8),
                    word("альфа", 0.85, 1.05),
                    word("лишнее", 1.1, 1.3),
                    word("—", 1.35, 1.45),
                    word("—", 1.5, 1.6),
                    word("—", 1.65, 1.75),
                ],
            ),
            utterance(
                0.0,
                2.0,
                1,
                vec![
                    word("альфа", 0.0, 0.3),
                    word("альфа", 0.35, 0.55),
                    word("альфа", 0.6, 0.8),
                    word("альфа", 0.85, 1.05),
                ],
            ),
        ];
        let changed = dedupe_words(&mut mixed_punct, &samples(), &ev, None);
        assert_eq!(changed, 1, "{mixed_punct:?}");
        assert_eq!(mixed_punct.len(), 1, "{mixed_punct:?}");
        assert_eq!(mixed_punct[0].channel, Some(1));
    }

    #[test]
    fn muffled_room_echo_is_suppressed_by_sys_rms() {
        let mut ev = evidence(&[(0.0, 2.0, FrameClass::Uncertain, 0.05)], 2.0);
        ev.prominence = 50.0;
        ev.gain = 0.15;
        ev.delay_sec = 0.0625;
        let audio = HashMap::from([
            (0usize, vec![0.06f32; 16_000 * 2]),
            (1usize, vec![0.20f32; 16_000 * 2]),
        ]);
        let mut utts = vec![
            utterance(0.2, 1.0, 0, vec![word("бета", 0.3, 0.8)]),
            utterance(0.0, 2.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let changed = dedupe_words(&mut utts, &audio, &ev, None);
        assert_eq!(changed, 1, "{utts:?}");
        assert_eq!(utts.len(), 1, "{utts:?}");
        assert_eq!(utts[0].channel, Some(1));

        let mut without_sys = vec![
            utterance(0.2, 1.0, 0, vec![word("бета", 0.3, 0.8)]),
            utterance(0.0, 2.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let mic_only = HashMap::from([(0usize, vec![0.06f32; 16_000 * 2])]);
        assert_eq!(dedupe_words(&mut without_sys, &mic_only, &ev, None), 0);
        assert_eq!(without_sys.len(), 2);
    }

    #[test]
    fn true_double_talk_with_local_only_is_preserved() {
        let mut ev = evidence(
            &[
                (0.0, 0.75, FrameClass::EchoOnly, 0.05),
                (0.75, 1.0, FrameClass::LocalOnly, 0.05),
            ],
            2.0,
        );
        ev.prominence = 50.0;
        ev.gain = 0.15;
        let audio = HashMap::from([
            (0usize, vec![0.06f32; 16_000 * 2]),
            (1usize, vec![0.20f32; 16_000 * 2]),
        ]);
        let mut utts = vec![
            utterance(0.0, 1.0, 0, vec![word("бета", 0.0, 1.0)]),
            utterance(0.0, 1.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let changed = dedupe_words(&mut utts, &audio, &ev, None);
        assert_eq!(changed, 0, "{utts:?}");
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[0].text, "бета");
    }
}
