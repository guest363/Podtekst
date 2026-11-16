use std::collections::{HashMap, HashSet};

use crate::transcribe::{Sample, SpeakerInfo, Utterance};

use super::text::{interval_overlap, text_similarity, word_count, words_contained};

struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[rb] = ra;
        }
    }
}

fn pair_evidence(by_name: &HashMap<&str, Vec<&Utterance>>, a: &str, b: &str) -> (f64, bool) {
    let mut overlap = 0.0;
    let mut echo = false;
    for u in by_name.get(a).into_iter().flatten() {
        for v in by_name.get(b).into_iter().flatten() {
            if u.channel == v.channel {
                continue;
            }
            let ov = interval_overlap(u.start, u.end, v.start, v.end);
            if ov > 0.0 {
                overlap += ov;
                if !echo && text_similarity(&u.text, &v.text) >= 0.55 {
                    echo = true;
                }
            }
        }
    }
    (overlap, echo)
}

/// Объединение спикеров, которые звучат в обоих каналах (эхо микрофона):
/// одинаковый голос + перекрывающиеся реплики. Возвращает канонические имена.
pub(crate) fn reconcile_channels(infos: &mut Vec<SpeakerInfo>, utterances: &mut [Utterance]) {
    let n = infos.len();
    if n < 2 {
        return;
    }
    let speakers_before = n;
    let by_name: HashMap<&str, Vec<&Utterance>> = {
        let mut map: HashMap<&str, Vec<&Utterance>> = HashMap::new();
        for u in utterances.iter() {
            map.entry(u.speaker.as_str()).or_default().push(u);
        }
        map
    };
    let mut dsu = Dsu::new(n);
    // Защита от транзитивного слияния разных распознанных профилей через промежуточные кластеры
    let mut matched_root: HashMap<usize, &str> = HashMap::new();
    for (idx, info) in infos.iter().enumerate() {
        if info.matched {
            matched_root.insert(idx, &info.name);
        }
    }
    let mut unnamed_roots: HashSet<usize> = infos
        .iter()
        .enumerate()
        .filter(|(_, info)| !info.matched && info.candidates.is_empty())
        .map(|(index, _)| index)
        .collect();
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = match (&infos[i].embedding, &infos[j].embedding) {
                (Some(a), Some(b)) => mr_ml::cosine_similarity(a, b),
                _ => 0.0,
            };
            if infos[i].matched ^ infos[j].matched {
                let (known, unknown) = if infos[i].matched {
                    (&infos[i], &infos[j])
                } else {
                    (&infos[j], &infos[i])
                };
                if !unknown.candidates.iter().any(|(name, score)| {
                    name == &known.name && *score >= mr_ml::DEFAULT_MATCH_THRESHOLD
                }) {
                    continue;
                }
            }
            let (overlap, echo) = pair_evidence(&by_name, &infos[i].name, &infos[j].name);
            let short = infos[i].utterances_count <= 2 || infos[j].utterances_count <= 2;
            let same_name = infos[i].name == infos[j].name;

            let should_merge =
                if infos[i].channel == infos[j].channel && infos[i].name != infos[j].name {
                    // Без профиля нет доказательства, что два кластера — один человек.
                    // Похожесть смешанных эмбеддингов сливала всех в один «средний» голос.
                    let (matched_info, unmatched_info) = if infos[i].matched {
                        (&infos[i], &infos[j])
                    } else {
                        (&infos[j], &infos[i])
                    };
                    let candidate_conflict =
                        unmatched_info
                            .candidates
                            .first()
                            .is_some_and(|(cand_name, score)| {
                                *score >= mr_ml::DEFAULT_MATCH_THRESHOLD * 0.90
                                    && cand_name.to_lowercase() != matched_info.name.to_lowercase()
                            });
                    (infos[i].matched ^ infos[j].matched) && sim >= 0.55 && !candidate_conflict
                } else {
                    // Между каналами того же человека выдают общее имя или
                    // одновременная речь (эхо); похожести без перекрытия мало —
                    // голоса разных людей тоже похожи.
                    same_name
                        || (sim >= 0.55 && overlap >= 0.3)
                        || (echo && sim >= 0.45 && overlap >= 0.3)
                        || (echo && short && overlap >= 0.8)
                };

            if should_merge {
                let ri = dsu.find(i);
                let rj = dsu.find(j);
                if let (Some(&name_i), Some(&name_j)) =
                    (matched_root.get(&ri), matched_root.get(&rj))
                {
                    if name_i != name_j {
                        continue; // Разные профили уже в этих компонентах — слияние строго запрещено!
                    }
                }
                if (matched_root.contains_key(&ri) && unnamed_roots.contains(&rj))
                    || (matched_root.contains_key(&rj) && unnamed_roots.contains(&ri))
                {
                    continue;
                }
                let unnamed = unnamed_roots.contains(&ri) || unnamed_roots.contains(&rj);
                dsu.union(i, j);
                let nr = dsu.find(i);
                if unnamed {
                    unnamed_roots.insert(nr);
                }
                if let Some(&name) = matched_root.get(&ri).or_else(|| matched_root.get(&rj)) {
                    matched_root.insert(nr, name);
                }
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = dsu.find(i);
        groups.entry(root).or_default().push(i);
    }
    let mut roots: Vec<usize> = groups.keys().copied().collect();
    roots.sort_unstable();
    let mut out = Vec::with_capacity(groups.len());
    let mut names: HashMap<String, String> = HashMap::new();
    let mut merged_count = 0usize;
    let mut cross_channel = 0usize;
    for root in roots {
        let members = groups.remove(&root).unwrap_or_default();
        if members.is_empty() {
            continue;
        }
        if members.len() > 1 {
            merged_count += members.len() - 1;
            let mut channels = HashSet::new();
            for &member in &members {
                channels.insert(infos[member].channel);
            }
            if channels.len() > 1 {
                cross_channel += 1;
            }
        }
        // Приоритет имени: узнанный профиль → системный канал → больше речи.
        let primary = members
            .iter()
            .copied()
            .find(|&m| infos[m].matched)
            .or_else(|| {
                members
                    .iter()
                    .copied()
                    .filter(|&m| infos[m].channel == Some(1))
                    .max_by(|&a, &b| {
                        infos[a]
                            .total_speech_sec
                            .total_cmp(&infos[b].total_speech_sec)
                    })
            })
            .or_else(|| {
                members.iter().copied().max_by(|&a, &b| {
                    infos[a]
                        .total_speech_sec
                        .total_cmp(&infos[b].total_speech_sec)
                })
            })
            .unwrap_or(members[0]);
        let canonical = infos[primary].name.clone();
        let mut merged = infos[primary].clone();
        merged.name = canonical.clone();
        merged.utterances_count = members.iter().map(|&m| infos[m].utterances_count).sum();
        merged.total_speech_sec = (members
            .iter()
            .map(|&m| infos[m].total_speech_sec)
            .sum::<f64>()
            * 10.0)
            .round()
            / 10.0;
        merged.similarity = members
            .iter()
            .map(|&m| infos[m].similarity)
            .fold(0.0, f64::max);
        let sil_weight: f64 = members.iter().map(|&m| infos[m].total_speech_sec).sum();
        if sil_weight > 0.0 {
            merged.silhouette = (members
                .iter()
                .map(|&m| infos[m].silhouette * infos[m].total_speech_sec)
                .sum::<f64>()
                / sil_weight
                * 1000.0)
                .round()
                / 1000.0;
        }
        merged.matched = members.iter().any(|&m| infos[m].matched);
        if merged.matched_name.is_none() {
            merged.matched_name = members.iter().find_map(|&m| infos[m].matched_name.clone());
        }
        let weighted: Vec<(Vec<f64>, f64)> = members
            .iter()
            .filter_map(|&m| {
                infos[m]
                    .embedding
                    .clone()
                    .map(|vector| (vector, infos[m].total_speech_sec.max(0.1)))
            })
            .collect();
        merged.embedding = if weighted.is_empty() {
            None
        } else {
            let (vecs, ws): (Vec<Vec<f64>>, Vec<f64>) = weighted.into_iter().unzip();
            Some(mr_ml::weighted_centroid(&vecs, &ws))
        };
        for &m in &members {
            for (name, score) in &infos[m].candidates {
                if !merged
                    .candidates
                    .iter()
                    .any(|(n, _)| n.to_lowercase() == name.to_lowercase())
                {
                    merged.candidates.push((name.clone(), *score));
                }
            }
            for s in &infos[m].samples {
                let duplicate = merged.samples.iter().any(|x| {
                    (x.start - s.start).abs() < 0.05
                        || (text_similarity(&x.text, &s.text) >= 0.75
                            && interval_overlap(x.start, x.end, s.start, s.end) > 0.2)
                });
                if !duplicate {
                    merged.samples.push(s.clone());
                }
            }
            names.insert(infos[m].name.clone(), canonical.clone());
        }
        // Фрагменты играем из основного канала: у системного канала нет
        // эха комнаты, у микрофонного — нет голоса собеседника.
        let primary_ch = infos[primary].channel;
        let primary_samples: Vec<Sample> = merged
            .samples
            .iter()
            .filter(|s| s.channel == primary_ch)
            .cloned()
            .collect();
        if !primary_samples.is_empty() {
            merged.samples = primary_samples;
        }
        merged.samples.sort_by(|a, b| a.start.total_cmp(&b.start));
        merged.candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.push(merged);
    }
    log::info!(
        target: "diar",
        speakers_before = speakers_before,
        speakers_after = out.len(),
        merged = merged_count,
        cross_channel = cross_channel;
        "ev=diar.reconcile"
    );
    *infos = out;
    for u in utterances.iter_mut() {
        if let Some(name) = names.get(&u.speaker) {
            u.speaker = name.clone();
        }
    }
}

/// Приклеить текст с учётом общего стыка: ASR мог распознать одни и те же
/// слова в перекрытии кусков. Срезаем самый длинный повторяющийся стык.
fn overlap_word_count(a: &str, b: &str) -> usize {
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

fn append_overlap_text(last: &mut String, next: &str) {
    let b: Vec<&str> = next.split_whitespace().collect();
    let k = overlap_word_count(last, next);
    if k < b.len() {
        last.push(' ');
        last.push_str(&b[k..].join(" "));
    }
}

fn utterance_rms(
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

fn better_utterance(a: &Utterance, b: &Utterance) -> bool {
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
