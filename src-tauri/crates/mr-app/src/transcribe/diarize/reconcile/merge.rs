use std::collections::{HashMap, HashSet};

use crate::transcribe::echo::{self, EchoEvidence};
use crate::transcribe::{SpeakerInfo, Utterance};

use super::super::text::{interval_overlap, text_similarity, words_contained};
use super::words::{append_overlap_text, better_utterance, overlap_word_count, utterance_rms};

const ECHO_PRECEDENCE_MAX_LOCAL_ONLY: f32 = 0.50;
const ECHO_PRECEDENCE_MAX_DOUBLE_TALK: f32 = 0.35;
const SHORT_ECHO_MIN_ECHO_ONLY: f32 = 0.60;
const SHORT_ECHO_MAX_LOCAL_ONLY: f32 = 0.05;
const SHORT_ECHO_MAX_DOUBLE_TALK: f32 = 0.15;
const SHORT_ECHO_LEAD_SEC: f64 = 0.1;
const SHORT_ECHO_TAIL_SEC: f64 = 0.3;

enum StrongEchoDecision {
    DropCh0,
    KeepBoth,
}

fn strong_echo_decision(
    u_ch0: &Utterance,
    u_ch1: &Utterance,
    sim: f64,
    ev: &EchoEvidence,
) -> StrongEchoDecision {
    let ch0_profile = echo::segment_profile(ev, u_ch0.start, u_ch0.end);
    if sim >= 0.55 {
        // При сильном эхе ch1 → ch0 источник физически известен; однако при
        // реальной одновременной речи оба спикера могли сказать одно и то же слово.
        if ch0_profile.local_only < ECHO_PRECEDENCE_MAX_LOCAL_ONLY
            && ch0_profile.double_talk < ECHO_PRECEDENCE_MAX_DOUBLE_TALK
        {
            StrongEchoDecision::DropCh0
        } else {
            StrongEchoDecision::KeepBoth
        }
    } else {
        let is_ch0_echo = ch0_profile.echo_only >= SHORT_ECHO_MIN_ECHO_ONLY
            && ch0_profile.local_only <= SHORT_ECHO_MAX_LOCAL_ONLY
            && ch0_profile.double_talk <= SHORT_ECHO_MAX_DOUBLE_TALK;
        let inside_ch1 = u_ch0.start >= u_ch1.start - SHORT_ECHO_LEAD_SEC
            && u_ch0.end <= u_ch1.end + SHORT_ECHO_TAIL_SEC;
        if is_ch0_echo && (words_contained(&u_ch0.text, &u_ch1.text) || inside_ch1) {
            StrongEchoDecision::DropCh0
        } else {
            StrongEchoDecision::KeepBoth
        }
    }
}

/// Одна и та же фраза, попавшая в оба канала (эхо), остаётся в одном виде.
/// Канал-источник определяется по уровню речи: пролезание в соседний канал
/// тише оригинала. Разные уверенно распознанные голоса при близких уровнях
/// не удаляем — иначе теряется один из спикеров.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn dedupe_cross_channel(
    utterances: &mut Vec<Utterance>,
    speakers: &[SpeakerInfo],
    channel_samples: &HashMap<usize, Vec<f32>>,
) {
    dedupe_cross_channel_with_echo(utterances, speakers, channel_samples, None);
}

pub(crate) fn dedupe_cross_channel_with_echo(
    utterances: &mut Vec<Utterance>,
    speakers: &[SpeakerInfo],
    channel_samples: &HashMap<usize, Vec<f32>>,
    echo: Option<&EchoEvidence>,
) {
    let before = utterances.len();
    if utterances.len() < 2 {
        return;
    }
    let strong_echo = echo.filter(|ev| ev.is_strong());
    let matched: HashSet<&str> = speakers
        .iter()
        .filter(|speaker| speaker.matched)
        .map(|speaker| speaker.name.as_str())
        .collect();
    let mut order: Vec<usize> = (0..utterances.len()).collect();
    order.sort_by(|&a, &b| utterances[a].start.total_cmp(&utterances[b].start));
    let mut keep = vec![true; utterances.len()];
    for (pos, &i) in order.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        for &j in &order[pos + 1..] {
            if utterances[j].start > utterances[i].end + 0.75 {
                break;
            }
            if !keep[j] || utterances[i].channel == utterances[j].channel {
                continue;
            }
            let ov = interval_overlap(
                utterances[i].start,
                utterances[i].end,
                utterances[j].start,
                utterances[j].end,
            );
            if ov <= 0.0 {
                continue;
            }
            let sim = text_similarity(&utterances[i].text, &utterances[j].text);
            if let Some(ev) = strong_echo {
                if let Some((ch0, ch1)) = match (utterances[i].channel, utterances[j].channel) {
                    (Some(0), Some(1)) => Some((i, j)),
                    (Some(1), Some(0)) => Some((j, i)),
                    _ => None,
                } {
                    match strong_echo_decision(&utterances[ch0], &utterances[ch1], sim, ev) {
                        StrongEchoDecision::DropCh0 => {
                            keep[ch0] = false;
                            if ch0 == i {
                                break;
                            }
                        }
                        StrongEchoDecision::KeepBoth => {}
                    }
                    continue;
                }
            }
            if sim < 0.55 {
                continue;
            }
            let rms_i = utterance_rms(
                channel_samples,
                &utterances[i],
                utterances[i].start.max(utterances[j].start),
                utterances[i].end.min(utterances[j].end),
            )
            .unwrap_or(0.0);
            let rms_j = utterance_rms(
                channel_samples,
                &utterances[j],
                utterances[i].start.max(utterances[j].start),
                utterances[i].end.min(utterances[j].end),
            )
            .unwrap_or(0.0);
            let loudest = rms_i.max(rms_j);
            let clear_level = loudest > 1e-6 && (rms_i - rms_j).abs() > loudest * 0.10;
            let both_matched = matched.contains(utterances[i].speaker.as_str())
                && matched.contains(utterances[j].speaker.as_str());
            let i_matched = matched.contains(utterances[i].speaker.as_str());
            let j_matched = matched.contains(utterances[j].speaker.as_str());
            if both_matched && utterances[i].speaker != utterances[j].speaker && !clear_level {
                continue;
            }
            let i_better = if i_matched != j_matched {
                i_matched
            } else if clear_level {
                rms_i > rms_j
            } else {
                better_utterance(&utterances[i], &utterances[j])
            };
            if i_better {
                keep[j] = false;
            } else {
                keep[i] = false;
                break;
            }
        }
    }
    let mut idx = 0usize;
    utterances.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });
    log::info!(
        target: "diar",
        utts_before = before,
        utts_after = utterances.len(),
        dropped = before - utterances.len();
        "ev=diar.dedupe"
    );
}

/// Склейка соседних реплик одного спикера. Перекрывающиеся дубли не
/// складываются в «два раза одно и то же», а заменяются лучшим вариантом.
pub(crate) fn merge_utterances(utterances: Vec<Utterance>) -> Vec<Utterance> {
    let before = utterances.len();
    let mut merged: Vec<Utterance> = Vec::with_capacity(utterances.len());
    for u in utterances {
        if let Some(last) = merged.last_mut() {
            if last.speaker == u.speaker {
                let last_dur = (last.end - last.start).max(0.0);
                let u_dur = (u.end - u.start).max(0.0);
                let combined_silhouette = (last.silhouette * last_dur + u.silhouette * u_dur)
                    / (last_dur + u_dur).max(f64::EPSILON);
                let ov = interval_overlap(last.start, last.end, u.start, u.end);
                let min_dur = last_dur.min(u_dur).max(0.1);
                // Перекрытие ≥25% — один и тот же фрагмент, попавший в оба
                // канала (или задвоенный ASR-сегмент). Оставляем лучший
                // вариант, а не склеиваем две копии текста.
                if ov / min_dur >= 0.25 {
                    // В одном канале перекрытие — это только padding на
                    // стыке кусков: текст разный, терять его нельзя.
                    // Эхо между каналами — дубль одной фразы, берём лучшую.
                    if u.channel == last.channel {
                        let k = overlap_word_count(&last.text, &u.text);
                        append_overlap_text(&mut last.text, &u.text);
                        if last.words.is_empty() || u.words.is_empty() {
                            last.words.clear();
                        } else {
                            last.words
                                .extend(u.words.get(k..).unwrap_or(&[]).iter().cloned());
                        }
                        last.ctc_used = last.ctc_used || u.ctc_used;
                        last.start = last.start.min(u.start);
                        last.end = last.end.max(u.end);
                    } else if better_utterance(&u, last) {
                        // Нельзя тянуть границы победителя задержкой отброшенного эха:
                        // у ch=1 интервал перекроет следующую реплику того же канала.
                        last.start = u.start;
                        last.end = u.end;
                        last.text.clone_from(&u.text);
                        last.channel = u.channel;
                        last.words.clone_from(&u.words);
                        last.ctc_used = u.ctc_used;
                    }
                    last.silhouette = combined_silhouette;
                    continue;
                }
                if u.start - last.end <= 1.0 {
                    if u.channel == last.channel {
                        last.text = format!("{} {}", last.text, u.text).trim().to_string();
                        if last.words.is_empty() || u.words.is_empty() {
                            last.words.clear();
                        } else {
                            last.words.extend(u.words.iter().cloned());
                        }
                        last.ctc_used = last.ctc_used || u.ctc_used;
                        last.end = last.end.max(u.end);
                        last.silhouette = combined_silhouette;
                        continue;
                    }
                    // Сквозь каналы не склеиваем и границы `last` не растягиваем;
                    // отбрасываем лишь вложенный хвост эха в другом канале.
                    if words_contained(&u.text, &last.text) {
                        continue;
                    }
                }
            }
        }
        merged.push(u);
    }
    log::debug!(
        target: "diar",
        before = before,
        after = merged.len(),
        merged = before - merged.len();
        "ev=diar.merge_utts"
    );
    merged
}

/// Пересчёт счётчиков после объединения/дедупликации: карточки не должны
/// показывать старые числа реплик и секунд.
pub(crate) fn refresh_speaker_stats(infos: &mut Vec<SpeakerInfo>, utterances: &[Utterance]) {
    let mut stats: HashMap<&str, (usize, f64)> = HashMap::new();
    for u in utterances {
        let e = stats.entry(u.speaker.as_str()).or_default();
        e.0 += 1;
        e.1 += (u.end - u.start).max(0.0);
    }
    for info in infos.iter_mut() {
        if let Some((count, sec)) = stats.get(info.name.as_str()) {
            info.utterances_count = *count;
            info.total_speech_sec = (*sec * 10.0).round() / 10.0;
        } else {
            info.utterances_count = 0;
            info.total_speech_sec = 0.0;
        }
    }
    infos.retain(|s| s.utterances_count > 0);
    for info in infos.iter() {
        log::info!(
            target: "diar",
            speaker_id = crate::logging::speaker_token(&info.name),
            utts = info.utterances_count,
            speech_sec = info.total_speech_sec,
            silhouette = info.silhouette,
            similarity = info.similarity,
            matched = info.matched;
            "ev=diar.stats"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utt(start: f64, end: f64, speaker: &str, ch: i32, text: &str) -> Utterance {
        Utterance {
            start,
            end,
            speaker: speaker.to_string(),
            channel: Some(ch),
            text: text.to_string(),
            diarized: true,
            silhouette: 0.5,
            words: Vec::new(),
            ctc_used: false,
        }
    }

    #[test]
    fn cross_channel_overlap_does_not_stretch_winner_into_next_same_channel_turn() {
        let utts = vec![
            utt(10.0, 12.0, "Спикер A", 1, "альфа бета гамма"),
            utt(10.2, 12.6, "Спикер A", 0, "альфа бета гамма"),
            utt(12.1, 14.0, "Спикер B", 1, "дельта эпсилон"),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 2, "{merged:?}");
        assert_eq!(merged[0].channel, Some(1));
        assert_eq!(merged[0].start, 10.0);
        assert_eq!(merged[0].end, 12.0);
        assert!(merged[0].end <= merged[1].start, "{merged:?}");
    }

    #[test]
    fn cross_channel_winner_keeps_its_own_bounds() {
        let utts = vec![
            utt(9.8, 12.0, "Спикер A", 0, "альфа бета"),
            utt(10.0, 12.3, "Спикер A", 1, "альфа бета гамма"),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(merged[0].channel, Some(1));
        assert_eq!(merged[0].start, 10.0);
        assert_eq!(merged[0].end, 12.3);
        assert_eq!(merged[0].text, "альфа бета гамма");
    }

    #[test]
    fn cross_channel_gap_never_joins_or_stretches_bounds() {
        let with_tail_echo = vec![
            utt(10.0, 12.0, "Спикер A", 1, "альфа бета гамма"),
            utt(12.1, 12.7, "Спикер A", 0, "гамма"),
            utt(12.2, 14.0, "Спикер B", 1, "дельта эпсилон"),
        ];
        let merged = merge_utterances(with_tail_echo);
        assert_eq!(merged.len(), 2, "{merged:?}");
        assert_eq!(merged[0].end, 12.0);
        assert!(merged[0].end <= merged[1].start);

        let distinct_other_channel = vec![
            utt(10.0, 12.0, "Спикер A", 1, "альфа бета"),
            utt(12.1, 12.7, "Спикер A", 0, "дельта"),
        ];
        let kept = merge_utterances(distinct_other_channel);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_eq!(kept[0].end, 12.0);
        assert_eq!(kept[1].channel, Some(0));
    }

    fn strong_echo(total_sec: f64, spans: &[(f64, f64, echo::FrameClass)]) -> EchoEvidence {
        let win = 512.0 / 16_000.0;
        let n = (total_sec / win).ceil() as usize;
        let mut frames = vec![echo::FrameClass::Uncertain; n];
        for (idx, frame) in frames.iter_mut().enumerate() {
            let t = idx as f64 * win;
            if let Some(&(_, _, class)) = spans.iter().find(|&&(a, b, _)| t >= a && t < b) {
                *frame = class;
            }
        }
        EchoEvidence {
            delay_samples: 1008,
            delay_sec: 1008.0 / 16_000.0,
            prominence: echo::STRONG_PROMINENCE + 20.0,
            stability: 1.0,
            gain: 0.4,
            noise_floor: 1e-4,
            residual: vec![0.0; n],
            frames,
        }
    }

    fn speaker_info(name: &str, ch: i32, matched: bool) -> SpeakerInfo {
        SpeakerInfo {
            id: format!("spk_{ch}"),
            name: name.to_string(),
            channel: Some(ch),
            matched,
            matched_name: matched.then(|| name.to_string()),
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: if matched { 0.8 } else { 0.0 },
            silhouette: 0.5,
            candidates: Vec::new(),
            embedding: None,
            samples: Vec::new(),
            utterances_count: 1,
            total_speech_sec: 2.0,
        }
    }

    #[test]
    fn dedupe_with_strong_echo_prefers_ch1_even_when_ch0_is_louder() {
        let ev = strong_echo(10.0, &[(0.0, 10.0, echo::FrameClass::EchoOnly)]);
        let samples = HashMap::from([
            (0usize, vec![0.5f32; 16_000 * 6]),
            (1usize, vec![0.1f32; 16_000 * 6]),
        ]);
        let speakers = vec![
            speaker_info("Спикер A", 0, true),
            speaker_info("Спикер B", 1, false),
        ];
        let mut utts = vec![
            utt(1.0, 3.0, "Спикер A", 0, "альфа бета гамма"),
            utt(1.0, 3.0, "Спикер B", 1, "альфа бета гамма"),
        ];
        dedupe_cross_channel_with_echo(&mut utts, &speakers, &samples, Some(&ev));
        assert_eq!(utts.len(), 1, "{utts:?}");
        assert_eq!(utts[0].channel, Some(1));
        assert_eq!(utts[0].speaker, "Спикер B");
    }

    #[test]
    fn dedupe_with_strong_echo_drops_short_ch0_echo_over_long_ch1_but_keeps_double_talk_backchannel(
    ) {
        let ev = strong_echo(
            20.0,
            &[
                (1.0, 3.0, echo::FrameClass::EchoOnly),
                (5.0, 7.0, echo::FrameClass::DoubleTalk),
                (9.0, 11.0, echo::FrameClass::LocalOnly),
            ],
        );
        let mut utts = vec![
            utt(
                0.0,
                15.0,
                "Спикер B",
                1,
                "альфа бета гамма дельта эпсилон зета эта тета йота каппа",
            ),
            utt(1.5, 2.3, "Спикер A", 0, "искаженное"),
            utt(5.5, 6.3, "Спикер A", 0, "понял"),
            utt(9.5, 10.3, "Спикер A", 0, "точно"),
        ];
        dedupe_cross_channel_with_echo(&mut utts, &[], &HashMap::new(), Some(&ev));
        let ch0_texts: Vec<&str> = utts
            .iter()
            .filter(|u| u.channel == Some(0))
            .map(|u| u.text.as_str())
            .collect();
        assert_eq!(ch0_texts, vec!["понял", "точно"], "{utts:?}");
        assert!(utts.iter().any(|u| u.channel == Some(1)));
    }

    #[test]
    fn dedupe_with_strong_echo_keeps_simultaneous_double_talk_same_word() {
        let ev = strong_echo(
            6.0,
            &[
                (1.0, 1.3, echo::FrameClass::EchoOnly),
                (1.3, 2.0, echo::FrameClass::DoubleTalk),
            ],
        );
        let samples = HashMap::from([
            (0usize, vec![0.4f32; 16_000 * 4]),
            (1usize, vec![0.1f32; 16_000 * 4]),
        ]);
        let mut utts = vec![
            utt(1.0, 2.0, "Спикер A", 0, "согласен"),
            utt(1.0, 2.0, "Спикер B", 1, "согласен"),
        ];
        dedupe_cross_channel_with_echo(&mut utts, &[], &samples, Some(&ev));
        assert_eq!(utts.len(), 2, "двойная речь не должна удаляться: {utts:?}");
        assert!(utts.iter().any(|u| u.channel == Some(0)));
        assert!(utts.iter().any(|u| u.channel == Some(1)));
    }
}
