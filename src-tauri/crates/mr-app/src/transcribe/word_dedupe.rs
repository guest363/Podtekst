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
/// Дрейф оценки задержки между зондами: ±60 мс перекрывают округление до кадра
/// (32 мс) и медленное расхождение часов каналов.
const DELAY_DRIFT_SEC: f64 = 0.06;

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
    let other_spans: Vec<(f64, f64)> = utterances
        .iter()
        .filter(|u| u.channel == Some(1))
        .map(|u| (u.start, u.end))
        .collect();
    let echo_words: Vec<(f64, f64, String)> = utterances
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

    let mut changed = 0usize;
    let mut kept: Vec<Utterance> = Vec::with_capacity(utterances.len());
    for mut utterance in utterances.drain(..) {
        if utterance.channel != Some(0)
            || !utterance.words.iter().any(|w| w.end_sec > w.start_sec)
            || !overlaps_any(&utterance, &other_spans)
        {
            kept.push(utterance);
            continue;
        }
        let local = local_words(&utterance, mic, echo, &echo_words);
        if local.len() == utterance.words.len() {
            kept.push(utterance);
            continue;
        }
        changed += 1;
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
    changed
}

fn overlaps_any(utterance: &Utterance, spans: &[(f64, f64)]) -> bool {
    spans
        .iter()
        .any(|&(start, end)| utterance.end.min(end) - utterance.start.max(start) > 0.0)
}

fn local_words(
    utterance: &Utterance,
    mic: &[f32],
    echo: &EchoEvidence,
    echo_words: &[(f64, f64, String)],
) -> Vec<WordConf> {
    utterance
        .words
        .iter()
        .filter(|word| !is_echo(word, mic, echo, echo_words))
        .cloned()
        .collect()
}

fn is_echo(
    word: &WordConf,
    mic: &[f32],
    echo: &EchoEvidence,
    echo_words: &[(f64, f64, String)],
) -> bool {
    if word.end_sec <= word.start_sec {
        return false;
    }
    let normalized = diarize::normalize_word(&word.w);
    if normalized.is_empty() {
        return false;
    }
    let profile = echo::segment_profile(echo, word.start_sec, word.end_sec);
    let rms = mic_rms(mic, word.start_sec, word.end_sec);
    let residual_ratio = echo::mean_residual(echo, word.start_sec, word.end_sec) / rms.max(1e-6);
    if profile.local_only > LOCAL_ONLY_SHARE
        || (profile.double_talk > DOUBLE_TALK_SHARE && residual_ratio > DOUBLE_TALK_RESIDUAL)
    {
        return false;
    }
    if residual_ratio > RESIDUAL_KEEP || profile.echo_only + profile.uncertain < ECHO_SHARE {
        return false;
    }
    matches_echo_word(
        &normalized,
        word.start_sec,
        word.end_sec,
        echo.delay_sec,
        echo_words,
    )
}

fn matches_echo_word(
    word: &str,
    start: f64,
    end: f64,
    delay_sec: f64,
    echo_words: &[(f64, f64, String)],
) -> bool {
    [
        delay_sec - DELAY_DRIFT_SEC,
        delay_sec,
        delay_sec + DELAY_DRIFT_SEC,
    ]
    .iter()
    .any(|shift| {
        echo_words.iter().any(|(other_start, other_end, other)| {
            let shifted_start = other_start + shift;
            let shifted_end = other_end + shift;
            shifted_end > start && shifted_start < end && diarize::words_match(word, other)
        })
    })
}

fn mic_rms(samples: &[f32], start: f64, end: f64) -> f32 {
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
    fn double_talk_with_residual_keeps_word() {
        let ev = evidence(&[(0.0, 2.0, FrameClass::DoubleTalk, 0.024)], 2.0);
        let mut utts = vec![
            utterance(0.0, 1.0, 0, vec![word("альфа", 0.0, 0.5)]),
            utterance(0.0, 1.0, 1, vec![word("альфа", 0.0, 0.5)]),
        ];
        let changed = dedupe_words(&mut utts, &samples(), &ev, None);
        assert_eq!(changed, 0, "{utts:?}");
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[0].text, "альфа");
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
}
