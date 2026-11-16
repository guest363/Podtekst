use std::collections::{HashMap, HashSet};

use crate::transcribe::{SpeakerInfo, Utterance};

use super::super::text::{interval_overlap, text_similarity, words_contained};
use super::words::{append_overlap_text, better_utterance, overlap_word_count, utterance_rms};

/// Одна и та же фраза, попавшая в оба канала (эхо), остаётся в одном виде.
/// Канал-источник определяется по уровню речи: пролезание в соседний канал
/// тише оригинала. Разные уверенно распознанные голоса при близких уровнях
/// не удаляем — иначе теряется один из спикеров.
pub(crate) fn dedupe_cross_channel(
    utterances: &mut Vec<Utterance>,
    speakers: &[SpeakerInfo],
    channel_samples: &HashMap<usize, Vec<f32>>,
) {
    let before = utterances.len();
    if utterances.len() < 2 {
        return;
    }
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
            if text_similarity(&utterances[i].text, &utterances[j].text) < 0.55 {
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
                    } else if better_utterance(&u, last) {
                        last.text = u.text.clone();
                        last.channel = u.channel;
                        last.words = u.words.clone();
                        last.ctc_used = u.ctc_used;
                    }
                    last.start = last.start.min(u.start);
                    last.end = last.end.max(u.end);
                    last.silhouette = combined_silhouette;
                    continue;
                }
                if u.start - last.end <= 1.0 {
                    // В одном канале соседние куски — это разные слова, даже если второй целиком повторяет
                    // слова первого; вложенность отсекается только для эха между разными каналами.
                    if u.channel == last.channel || !words_contained(&u.text, &last.text) {
                        last.text = format!("{} {}", last.text, u.text).trim().to_string();
                        if last.words.is_empty() || u.words.is_empty() {
                            last.words.clear();
                        } else {
                            last.words.extend(u.words.iter().cloned());
                        }
                        last.ctc_used = last.ctc_used || u.ctc_used;
                    }
                    last.end = last.end.max(u.end);
                    last.silhouette = combined_silhouette;
                    continue;
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
