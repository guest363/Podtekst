use std::collections::{HashMap, HashSet};

use crate::transcribe::{Sample, SpeakerInfo, Utterance};

use super::super::text::{interval_overlap, text_similarity};

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
    // Каналы безымянных кластеров в компоненте: мешает транзитивному слиянию
    // двух разных собеседников ch1 через общий кластер эха в ch0.
    let mut unmatched_channels: HashMap<usize, HashSet<Option<i32>>> = infos
        .iter()
        .enumerate()
        .filter(|(_, info)| !info.matched)
        .map(|(index, info)| (index, HashSet::from([info.channel])))
        .collect();
    let mut candidates: Vec<(bool, f64, f64, usize, usize)> = Vec::new();
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
            // Короткий эхо-кластер возможен только со стороны микрофона (ch=0):
            // иначе короткая реплика собеседника (ch=1) переименует весь кластер «Я».
            let short_mic_echo = (infos[i].channel == Some(0) && infos[i].utterances_count <= 2)
                || (infos[j].channel == Some(0) && infos[j].utterances_count <= 2);
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
                        || (echo && short_mic_echo && overlap >= 0.8)
                };

            if should_merge {
                candidates.push((same_name, sim, overlap, i, j));
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| b.2.total_cmp(&a.2))
            .then_with(|| a.3.cmp(&b.3))
            .then_with(|| a.4.cmp(&b.4))
    });
    for (_, _, _, i, j) in candidates {
        let ri = dsu.find(i);
        let rj = dsu.find(j);
        if ri == rj {
            continue;
        }
        if let (Some(&name_i), Some(&name_j)) = (matched_root.get(&ri), matched_root.get(&rj)) {
            if name_i != name_j {
                continue; // Разные профили уже в этих компонентах — слияние строго запрещено!
            }
        }
        if (matched_root.contains_key(&ri) && unnamed_roots.contains(&rj))
            || (matched_root.contains_key(&rj) && unnamed_roots.contains(&ri))
        {
            continue;
        }
        if !matched_root.contains_key(&ri) && !matched_root.contains_key(&rj) {
            let clash = unmatched_channels
                .get(&ri)
                .zip(unmatched_channels.get(&rj))
                .is_some_and(|(ci, cj)| !ci.is_disjoint(cj));
            if clash {
                continue;
            }
        }
        let unnamed = unnamed_roots.contains(&ri) || unnamed_roots.contains(&rj);
        let mut merged_ch = unmatched_channels.remove(&ri).unwrap_or_default();
        if let Some(other_ch) = unmatched_channels.remove(&rj) {
            merged_ch.extend(other_ch);
        }
        let matched_name = matched_root
            .get(&ri)
            .or_else(|| matched_root.get(&rj))
            .copied();
        dsu.union(i, j);
        let nr = dsu.find(i);
        if !merged_ch.is_empty() {
            unmatched_channels.insert(nr, merged_ch);
        }
        if unnamed {
            unnamed_roots.insert(nr);
        }
        if let Some(name) = matched_name {
            matched_root.insert(nr, name);
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
