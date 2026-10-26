//! Диаризация: субсегментация, кластеризация канала
//! и сопоставление спикеров с профилями Voice Packs.

use std::collections::{HashMap, HashSet};

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::{interval_gap, match_voiceprints_matrix, SpeakerProfile};
use mr_packs as packs;

use super::{Sample, SpeakerInfo, Utterance};

/// Группировка VAD-сегментов в окна для ASR (≤ `max_dur`, склейка по паузе ≤ `gap`).
pub(crate) fn group_segments(segments: &[(f32, f32)], max_dur: f32, gap: f32) -> Vec<(f32, f32)> {
    let mut norm: Vec<(f32, f32)> = Vec::new();
    for &(s, e) in segments {
        let dur = e - s;
        if dur <= max_dur {
            norm.push((s, e));
        } else {
            let n = (dur / max_dur).ceil() as usize;
            let step = dur / n as f32;
            for i in 0..n {
                let a = s + i as f32 * step;
                let b = if i + 1 == n {
                    e
                } else {
                    s + (i + 1) as f32 * step
                };
                norm.push((a, b));
            }
        }
    }
    let mut grouped: Vec<(f32, f32)> = Vec::new();
    for (s, e) in norm {
        if let Some(last) = grouped.last_mut() {
            if e - last.0 <= max_dur && s - last.1 <= gap {
                last.1 = e;
                continue;
            }
        }
        grouped.push((s, e));
    }
    grouped
}

/// Склейка тихих коротких фрагментов через более длинную паузу:
/// по отдельности ASR теряет тихую короткую речь. Громкую и длинную речь не трогаем.
pub(crate) fn merge_quiet_short(samples: &[f32], groups: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    const SHORT_SEC: f32 = 2.0;
    const MAX_MERGED_SEC: f32 = 12.0;
    const QUIET_RMS: f32 = 0.02;
    const MAX_GAP_SEC: f32 = 3.0;
    let rms = |a: f32, b: f32| -> f32 {
        let lo = (a.max(0.0) * 16000.0) as usize;
        let hi = ((b.max(0.0) * 16000.0) as usize).min(samples.len());
        if hi <= lo {
            return 0.0;
        }
        let s = &samples[lo..hi];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    let mut out: Vec<(f32, f32)> = Vec::new();
    for (s, e) in groups {
        if let Some(last) = out.last_mut() {
            let gap = s - last.1;
            let incoming_short = e - s <= SHORT_SEC;
            if incoming_short
                && (0.0..=MAX_GAP_SEC).contains(&gap)
                && e - last.0 <= MAX_MERGED_SEC
                && rms(last.0, last.1) < QUIET_RMS
                && rms(s, e) < QUIET_RMS
            {
                last.1 = e;
                continue;
            }
        }
        out.push((s, e));
    }
    out
}

/// Лучшие образцы речи спикера для карточки в UI.
fn pick_samples(utterances: &[Utterance], max_samples: usize) -> Vec<Sample> {
    let mut scored: Vec<(f64, &Utterance)> = utterances
        .iter()
        .map(|u| {
            let dur = (u.end - u.start).max(0.0);
            let words = u.text.split_whitespace().count() as f64;
            let dur_score = 1.0 - ((dur - 3.8).abs() / 3.8).min(1.0);
            (dur_score * 2.0 + (words / 10.0).min(1.0), u)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(max_samples)
        .map(|(_, u)| {
            let dur = (u.end - u.start).max(0.5);
            let start = (u.start * 100.0).round() / 100.0;
            let sample_dur = (dur.min(6.0) * 10.0).round() / 10.0;
            let text = u.text.trim();
            let preview = if text.chars().count() > 98 {
                format!("{}…", text.chars().take(95).collect::<String>())
            } else {
                text.to_string()
            };
            Sample {
                start,
                end: ((start + sample_dur) * 100.0).round() / 100.0,
                dur: sample_dur,
                text: preview,
                full_text: text.to_string(),
                channel: u.channel,
            }
        })
        .collect()
}

/// Прочитать один `voiceprints.json`, отбросив чужое векторное пространство
/// и битые размерности. Профили без центроида, но с прототипами, собираются
/// из прототипов: иначе старый/импортированный пакет молча выпадает из матчинга.
fn read_profiles_file(file: &std::path::Path) -> Vec<SpeakerProfile> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    if let Some(space) = v.get("embedding_space").and_then(|x| x.as_str()) {
        if space != mr_ml::EMBEDDING_SPACE {
            return vec![];
        }
    }
    let arr = if let Some(a) = v.get("profiles").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.as_array() {
        a.clone()
    } else {
        return vec![];
    };
    let mut out = vec![];
    for p in arr {
        let Ok(mut prof) = serde_json::from_value::<SpeakerProfile>(p) else {
            continue;
        };
        if prof.name.trim().is_empty() {
            continue;
        }
        prof.prototypes
            .retain(|x| x.embedding.len() == mr_ml::EMBEDDING_DIM);
        if prof.embedding.len() != mr_ml::EMBEDDING_DIM
            || prof.embedding.iter().all(|x| x.abs() < 1e-12)
        {
            let vecs: Vec<Vec<f64>> = prof
                .prototypes
                .iter()
                .map(|x| x.embedding.clone())
                .collect();
            let ws: Vec<f64> = prof
                .prototypes
                .iter()
                .map(|x| x.duration_sec.max(0.5) * x.quality.max(0.1))
                .collect();
            prof.embedding = mr_ml::weighted_centroid(&vecs, &ws);
        }
        if prof.embedding.len() == mr_ml::EMBEDDING_DIM
            && prof.embedding.iter().any(|x| x.abs() > 1e-12)
        {
            out.push(prof);
        }
    }
    out
}

/// Профили спикеров из включённых Voice Packs (v2/v3 + legacy-массив).
/// Корневой `voiceprints.json` читается первым как fallback: активные паки
/// важнее старых локальных данных.
pub(crate) fn load_profiles() -> Vec<SpeakerProfile> {
    let app = packs::default_app_data_dir();
    let mut files = vec![app.join("voiceprints.json")];
    files.extend(
        packs::get_active_packs(&app)
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.join("voiceprints.json")),
    );
    let mut by_name: HashMap<String, SpeakerProfile> = HashMap::new();
    for f in files {
        for prof in read_profiles_file(&f) {
            by_name.insert(prof.name.trim().to_lowercase(), prof);
        }
    }
    let mut out: Vec<SpeakerProfile> = by_name.into_values().collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Минимальная длительность реплики для core-кластеризации, с.
const CORE_MIN_DUR: f64 = 1.4;

fn rms_quality(chunk: &[f32]) -> f64 {
    if chunk.is_empty() {
        return 0.5;
    }
    let rms = (chunk.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / chunk.len() as f64).sqrt();
    (rms / 0.05).clamp(0.3, 1.5)
}

/// Диаризация одного канала: эмбеддинги → core-кластеризация → short/micro-привязка
/// → глобальное сопоставление с профилями (Венгерский + Margin Gate).
#[allow(clippy::too_many_arguments)]
pub(crate) fn diarize_channel(
    samples: &[f32],
    utterances: &mut [Utterance],
    channel_idx: Option<i32>,
    default_name: &str,
    prefix: &str,
    extractor: &mut Option<EmbeddingExtractor>,
    profiles: &[SpeakerProfile],
    min_speakers: usize,
    max_speakers: usize,
    on_event: &dyn Fn(u8, &str),
) -> Vec<SpeakerInfo> {
    use mr_ml::{cluster::agglomerative, cosine_similarity};
    let sr = 16000.0;
    let n = utterances.len();
    if n == 0 {
        return vec![];
    }
    let mut embs: Vec<Option<Vec<f64>>> = vec![None; n];
    let mut durs = vec![0.0f64; n];
    let mut quals = vec![1.0f64; n];
    for (i, u) in utterances.iter().enumerate() {
        let dur = (u.end - u.start).max(0.0);
        durs[i] = dur;
        let lo = (u.start.max(0.0) * sr) as usize;
        let hi = ((u.end.max(0.0) * sr) as usize).min(samples.len());
        if hi > lo && hi - lo >= (sr * 0.4) as usize {
            if let Some(ext) = extractor {
                if let Ok(Some(e)) = ext.embed(&samples[lo..hi]) {
                    quals[i] = rms_quality(&samples[lo..hi]);
                    embs[i] = Some(e);
                }
            }
        }
        // Векторы голосов — самый долгий шаг после ASR: без счётчика
        // прогресс замирал на 79% и расшифровка выглядела зависшей.
        if i % 20 == 0 || i + 1 == n {
            on_event(
                (80 + 8 * i / n.max(1)) as u8,
                &format!("Определяю спикеров: {i} из {n} реплик"),
            );
        }
    }
    let valid: Vec<usize> = (0..n).filter(|&i| embs[i].is_some()).collect();
    if valid.is_empty() {
        for u in utterances.iter_mut() {
            u.speaker = default_name.to_string();
            u.diarized = true;
        }
        return vec![SpeakerInfo {
            id: "channel_x".to_string(),
            name: default_name.to_string(),
            channel: channel_idx,
            matched: false,
            matched_name: None,
            similarity: 0.0,
            candidates: vec![],
            embedding: None,
            samples: vec![],
            utterances_count: utterances.len(),
            total_speech_sec: durs.iter().sum(),
        }];
    }
    // Core-кластеризация по длинным репликам
    let mut core: Vec<usize> = valid
        .iter()
        .copied()
        .filter(|&i| durs[i] >= CORE_MIN_DUR)
        .collect();
    if core.is_empty() {
        core = valid.clone();
    }
    let core_vecs: Vec<Vec<f64>> = core.iter().map(|&i| embs[i].clone().unwrap()).collect();
    let k = if min_speakers > 0 && min_speakers == max_speakers {
        Some(min_speakers)
    } else {
        None
    };
    // Калибровка ERes2NetV2: кросс-спикерные пары ≤ 0.22, один голос ≥ 0.48 (от 1.5 с)
    let mut core_labels = agglomerative(&core_vecs, 0.45, k);
    // Уважение min/max при авто-режиме
    if k.is_none() {
        let mut uniq: Vec<usize> = core_labels.clone();
        uniq.sort();
        uniq.dedup();
        if max_speakers > 0 && uniq.len() > max_speakers {
            core_labels = agglomerative(&core_vecs, f64::INFINITY, Some(max_speakers));
        } else if min_speakers > 1 && uniq.len() < min_speakers && core_vecs.len() >= min_speakers {
            core_labels = agglomerative(&core_vecs, f64::INFINITY, Some(min_speakers));
        }
    }
    on_event(88, "Голоса разделены по спикерам");
    let mut label_of: HashMap<usize, usize> = core
        .iter()
        .copied()
        .zip(core_labels.iter().copied())
        .collect();
    // Short-привязка с темпоральным бонусом
    let recompute = |label_of: &HashMap<usize, usize>| -> HashMap<usize, Vec<f64>> {
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for &i in valid.iter() {
            if let Some(&l) = label_of.get(&i) {
                groups.entry(l).or_default().push(i);
            }
        }
        groups
            .into_iter()
            .map(|(l, members)| {
                let vecs: Vec<Vec<f64>> =
                    members.iter().map(|&i| embs[i].clone().unwrap()).collect();
                let ws: Vec<f64> = members
                    .iter()
                    .map(|&i| durs[i].min(8.0) * quals[i])
                    .collect();
                (l, mr_ml::weighted_centroid(&vecs, &ws))
            })
            .collect()
    };
    let mut centroids = recompute(&label_of);
    let mut core_sorted = core.clone();
    core_sorted.sort_by(|a, b| utterances[*a].start.total_cmp(&utterances[*b].start));
    for &si in valid.iter().filter(|i| !core.contains(i)) {
        let se = embs[si].clone().unwrap();
        let (s0, e0) = (utterances[si].start, utterances[si].end);
        let mut best = (i32::MIN, 0usize, -2.0f64);
        for (&lbl, cent) in centroids.iter() {
            let mut score = cosine_similarity(&se, cent);
            for &ci in core_sorted
                .iter()
                .filter(|&&ci| label_of.get(&ci) == Some(&lbl))
            {
                if interval_gap(s0, e0, utterances[ci].start, utterances[ci].end) <= 0.6 {
                    score += 0.12;
                    break;
                }
            }
            if score > best.2 {
                best = (0, lbl, score);
            }
        }
        label_of.insert(si, best.1);
        centroids = recompute(&label_of);
    }
    // Микро-реплики — по зазору границ
    for i in 0..n {
        if label_of.contains_key(&i) {
            continue;
        }
        let (s0, e0) = (utterances[i].start, utterances[i].end);
        let mut best = (f64::INFINITY, 0usize);
        for &v in valid.iter() {
            let g = interval_gap(s0, e0, utterances[v].start, utterances[v].end);
            if g < best.0 {
                best = (g, label_of[&v]);
            }
        }
        label_of.insert(i, best.1);
    }
    // Глобальное сопоставление кластеров с профилями
    let mut raw_clusters: Vec<usize> = label_of.values().copied().collect();
    raw_clusters.sort();
    raw_clusters.dedup();
    let ordered_centroids: Vec<Vec<f64>> = raw_clusters
        .iter()
        .map(|c| {
            let members: Vec<usize> = label_of
                .iter()
                .filter(|(_, &l)| l == *c)
                .map(|(&i, _)| i)
                .collect();
            let vecs: Vec<Vec<f64>> = members.iter().filter_map(|&i| embs[i].clone()).collect();
            let ws: Vec<f64> = members
                .iter()
                .map(|&i| durs[i].min(8.0) * quals[i])
                .collect();
            if vecs.is_empty() {
                vec![0.0; mr_ml::EMBEDDING_DIM]
            } else {
                mr_ml::weighted_centroid(&vecs, &ws)
            }
        })
        .collect();
    let matches = match_voiceprints_matrix(
        &ordered_centroids,
        profiles,
        mr_ml::DEFAULT_MATCH_THRESHOLD,
        mr_ml::DEFAULT_MIN_MARGIN,
    );
    // Имена: совпавшие — из профилей, остальные — префикс по счёту
    let mut cluster_name: HashMap<usize, String> = HashMap::new();
    let mut unmatched = vec![];
    for (k, &c) in raw_clusters.iter().enumerate() {
        if matches[k].matched {
            cluster_name.insert(c, matches[k].name.clone().unwrap());
        } else {
            unmatched.push(c);
        }
    }
    unmatched.sort_by(|&a, &b| {
        let ca = label_of.values().filter(|&&l| l == a).count();
        let cb = label_of.values().filter(|&&l| l == b).count();
        cb.cmp(&ca).then(
            label_of
                .iter()
                .filter(|(_, &l)| l == b)
                .map(|(&i, _)| utterances[i].start)
                .fold(f64::INFINITY, f64::min)
                .partial_cmp(
                    &label_of
                        .iter()
                        .filter(|(_, &l)| l == a)
                        .map(|(&i, _)| utterances[i].start)
                        .fold(f64::INFINITY, f64::min),
                )
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    let generic = [
        "Собеседник",
        "Собеседники",
        "Участник",
        "Участники",
        "Other",
    ];
    if raw_clusters.len() == 1 && unmatched.len() == 1 {
        cluster_name.insert(raw_clusters[0], default_name.to_string());
    } else {
        let mut used: std::collections::HashSet<String> = cluster_name.values().cloned().collect();
        let default_used = used.contains(default_name);
        let mut next_num = 1usize;
        for (idx, c) in unmatched.into_iter().enumerate() {
            let name = if idx == 0 && !default_used && !generic.contains(&default_name) {
                default_name.to_string()
            } else {
                while used.contains(&format!("{prefix} {next_num}")) {
                    next_num += 1;
                }
                let name = format!("{prefix} {next_num}");
                next_num += 1;
                name
            };
            used.insert(name.clone());
            cluster_name.insert(c, name);
        }
    }
    let mut unique: Vec<String> = vec![];
    for c in raw_clusters.iter() {
        let nm = &cluster_name[c];
        if !unique.contains(nm) {
            unique.push(nm.clone());
        }
    }
    let mut infos = vec![];
    for name in unique {
        let cids: Vec<usize> = raw_clusters
            .iter()
            .copied()
            .filter(|c| cluster_name[c] == name)
            .collect();
        let members: Vec<usize> = label_of
            .iter()
            .filter(|(_, &l)| cids.contains(&l))
            .map(|(&i, _)| i)
            .collect();
        if members.is_empty() {
            continue;
        }
        let is_matched = cids.iter().any(|c| {
            let k = raw_clusters
                .iter()
                .position(|x| x == c)
                .expect("c из raw_clusters");
            matches[k].matched && matches[k].name.as_deref() == Some(&name)
        });
        let sim = cids
            .iter()
            .map(|c| {
                matches[raw_clusters
                    .iter()
                    .position(|x| x == c)
                    .expect("c из raw_clusters")]
                .similarity
            })
            .fold(0.0f64, f64::max);
        let mut cands = vec![];
        for c in cids.iter() {
            let k = raw_clusters
                .iter()
                .position(|x| x == c)
                .expect("c из raw_clusters");
            for mc in &matches[k].candidates {
                if !cands.iter().any(|(n, _): &(String, f64)| n == &mc.name) {
                    cands.push((mc.name.clone(), mc.similarity));
                }
            }
        }
        let member_utts: Vec<Utterance> = members.iter().map(|&i| utterances[i].clone()).collect();
        let samples = pick_samples(&member_utts, 3);
        let emb = {
            let vecs: Vec<Vec<f64>> = members.iter().filter_map(|&i| embs[i].clone()).collect();
            let ws: Vec<f64> = members
                .iter()
                .map(|&i| durs[i].min(8.0) * quals[i])
                .collect();
            if vecs.is_empty() {
                None
            } else {
                Some(mr_ml::weighted_centroid(&vecs, &ws))
            }
        };
        let spk_id = if channel_idx == Some(0) && name == default_name {
            "speaker_me".to_string()
        } else if let Some(ch) = channel_idx {
            format!("spk_ch{ch}_{name}")
        } else {
            format!("spk_{name}")
        };
        infos.push(SpeakerInfo {
            id: spk_id,
            name: name.clone(),
            channel: channel_idx,
            matched: is_matched,
            matched_name: if is_matched { Some(name.clone()) } else { None },
            similarity: (sim * 1000.0).round() / 1000.0,
            candidates: cands,
            embedding: emb.filter(|v| v.iter().any(|&x| x.abs() > 1e-12)),
            samples,
            utterances_count: members.len(),
            total_speech_sec: (members.iter().map(|&i| durs[i]).sum::<f64>() * 10.0).round() / 10.0,
        });
    }
    for (i, u) in utterances.iter_mut().enumerate() {
        u.speaker = cluster_name[&label_of[&i]].clone();
        u.diarized = true;
    }
    infos
}

/// Слова для сравнения текстов: пунктуация и регистр не важны.
fn words_of(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn words_contained(small: &str, big: &str) -> bool {
    let a = words_of(small);
    let b = words_of(big);
    !a.is_empty() && a.is_subset(&b)
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
    // ASR в двух каналах может добавить/потерять служебные слова.
    if wa.is_subset(&wb) || wb.is_subset(&wa) {
        j.max(0.85)
    } else {
        j
    }
}

fn interval_overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

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

/// Суммарное перекрытие реплик двух спикеров и признак «та же фраза».
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
    let by_name: HashMap<&str, Vec<&Utterance>> = {
        let mut map: HashMap<&str, Vec<&Utterance>> = HashMap::new();
        for u in utterances.iter() {
            map.entry(u.speaker.as_str()).or_default().push(u);
        }
        map
    };
    let mut dsu = Dsu::new(n);
    for i in 0..n {
        for j in (i + 1)..n {
            if infos[i].channel == infos[j].channel && infos[i].name != infos[j].name {
                continue;
            }
            // Разные профили — это разные люди, даже если голоса похожи.
            if infos[i].matched && infos[j].matched && infos[i].name != infos[j].name {
                continue;
            }
            let sim = match (&infos[i].embedding, &infos[j].embedding) {
                (Some(a), Some(b)) => mr_ml::cosine_similarity(a, b),
                _ => 0.0,
            };
            let (overlap, echo) = pair_evidence(&by_name, &infos[i].name, &infos[j].name);
            let short = infos[i].utterances_count <= 2 || infos[j].utterances_count <= 2;
            let same_name = infos[i].name == infos[j].name;
            let should_merge = same_name
                || (sim >= 0.55 && overlap >= 0.3)
                || (sim >= 0.75 && !infos[i].matched && !infos[j].matched)
                || ((infos[i].matched || infos[j].matched) && sim >= 0.50 && overlap >= 0.3)
                || ((infos[i].matched ^ infos[j].matched) && sim >= 0.65)
                || ((infos[i].matched ^ infos[j].matched) && echo && overlap >= 0.5)
                || (echo && sim >= 0.45 && overlap >= 0.3)
                || (echo && short && overlap >= 0.8);
            if should_merge {
                dsu.union(i, j);
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
    for root in roots {
        let members = groups.remove(&root).unwrap_or_default();
        if members.is_empty() {
            continue;
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
        merged.matched = members.iter().any(|&m| infos[m].matched);
        if merged.matched_name.is_none() {
            merged.matched_name = members.iter().find_map(|&m| infos[m].matched_name.clone());
        }
        if merged.embedding.is_none() {
            merged.embedding = members.iter().find_map(|&m| infos[m].embedding.clone());
        }
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
    *infos = out;
    for u in utterances.iter_mut() {
        if let Some(name) = names.get(&u.speaker) {
            u.speaker = name.clone();
        }
    }
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
pub(crate) fn dedupe_cross_channel(utterances: &mut Vec<Utterance>) {
    if utterances.len() < 2 {
        return;
    }
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
            if !keep[j]
                || utterances[i].channel == utterances[j].channel
                || utterances[i].speaker != utterances[j].speaker
            {
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
            if better_utterance(&utterances[j], &utterances[i]) {
                keep[i] = false;
                break;
            }
            keep[j] = false;
        }
    }
    let mut idx = 0usize;
    utterances.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });
}

/// Склейка соседних реплик одного спикера. Перекрывающиеся дубли не
/// складываются в «два раза одно и то же», а заменяются лучшим вариантом.
pub(crate) fn merge_utterances(utterances: Vec<Utterance>) -> Vec<Utterance> {
    let mut merged: Vec<Utterance> = Vec::with_capacity(utterances.len());
    for u in utterances {
        if let Some(last) = merged.last_mut() {
            if last.speaker == u.speaker {
                let ov = interval_overlap(last.start, last.end, u.start, u.end);
                let min_dur = (last.end - last.start).min(u.end - u.start).max(0.1);
                // Перекрытие ≥25% — один и тот же фрагмент, попавший в оба
                // канала (или задвоенный ASR-сегмент). Оставляем лучший
                // вариант, а не склеиваем две копии текста.
                if ov / min_dur >= 0.25 {
                    if better_utterance(&u, last) {
                        last.text = u.text.clone();
                        last.channel = u.channel;
                    }
                    last.start = last.start.min(u.start);
                    last.end = last.end.max(u.end);
                    continue;
                }
                if u.start - last.end <= 1.0 {
                    if !words_contained(&u.text, &last.text) {
                        last.text = format!("{} {}", last.text, u.text).trim().to_string();
                    }
                    last.end = last.end.max(u.end);
                    continue;
                }
            }
        }
        merged.push(u);
    }
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
}

/// Разрез длинных реплик при смене голоса: скользящее окно эмбеддингов 1.6 с /
/// шаг 0.5 с; разрез при косинусном сходстве < 0.35 (локальный минимум; в
/// пространстве eres2netv2-192-v1 один голос даёт ~0.5+, смена диктора ≤ 0.22).
pub(crate) fn split_long_by_speaker_change(
    samples: &[f32],
    utterances: Vec<Utterance>,
    extractor: &mut Option<EmbeddingExtractor>,
) -> Vec<Utterance> {
    let Some(ext) = extractor else {
        return utterances;
    };
    let sr = 16000.0;
    let mut out = Vec::new();
    for u in utterances {
        let dur = u.end - u.start;
        if dur < 3.2 {
            out.push(u);
            continue;
        }
        let mut wins: Vec<(f64, Vec<f64>)> = vec![];
        let mut t = u.start;
        while t + 1.6 <= u.end + 1e-6 {
            let lo = (t * sr) as usize;
            let hi = ((t + 1.6).min(u.end) * sr) as usize;
            if hi > lo && hi <= samples.len() {
                if let Ok(Some(e)) = ext.embed(&samples[lo..hi]) {
                    wins.push((t, e));
                }
            }
            t += 0.5;
        }
        if wins.len() < 3 {
            out.push(u);
            continue;
        }
        let sims: Vec<f64> = wins
            .windows(2)
            .map(|w| mr_ml::cosine_similarity(&w[0].1, &w[1].1))
            .collect();
        let mut cuts = vec![];
        for (i, &s) in sims.iter().enumerate() {
            if s < 0.35 {
                let prev = if i > 0 { sims[i - 1] } else { 1.0 };
                let next = sims.get(i + 1).copied().unwrap_or(1.0);
                if s <= prev && s <= next {
                    let b =
                        ((wins[i].0 + wins[i + 1].0) / 2.0 + 0.8).clamp(u.start + 0.5, u.end - 0.5);
                    if cuts.last().map(|&c| b - c >= 1.0).unwrap_or(true) {
                        cuts.push((b * 100.0).round() / 100.0);
                    }
                }
            }
        }
        if cuts.is_empty() {
            out.push(u);
            continue;
        }
        let words: Vec<&str> = u.text.split_whitespace().collect();
        let mut bounds = vec![u.start];
        bounds.extend(cuts);
        bounds.push(u.end);
        let num_segments = bounds.len() - 1;
        if words.len() < num_segments {
            out.push(u);
            continue;
        }
        for (text, w) in split_words_by_bounds(&words, &bounds)
            .into_iter()
            .zip(bounds.windows(2))
        {
            if text.is_empty() {
                continue;
            }
            out.push(Utterance {
                start: w[0],
                end: w[1],
                speaker: u.speaker.clone(),
                channel: u.channel,
                text,
                diarized: false,
            });
        }
    }
    out
}

/// Раскладка слов реплики по окнам между границами (доля окна от общей
/// длительности ≈ доля слов). Правая граница среза — от длины массива,
/// а не остатка, иначе при `lo > words.len()/2` срез паниковал.
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
            words.len().saturating_sub(pos)
        } else {
            ((words.len() as f64 * frac).round() as usize).max(1)
        };
        let lo = pos.min(words.len());
        let hi = (lo + nw).min(words.len());
        texts.push(words[lo..hi].join(" "));
        pos += nw;
    }
    texts
}

#[cfg(test)]
mod tests {
    use super::{
        dedupe_cross_channel, merge_quiet_short, merge_utterances, read_profiles_file,
        reconcile_channels, refresh_speaker_stats, split_words_by_bounds, SpeakerInfo, Utterance,
    };

    fn info(name: &str, ch: i32, emb: Vec<f64>, count: usize, sec: f64) -> SpeakerInfo {
        SpeakerInfo {
            id: format!("spk_{name}"),
            name: name.to_string(),
            channel: Some(ch),
            matched: false,
            matched_name: None,
            similarity: 0.0,
            candidates: vec![],
            embedding: Some(emb),
            samples: vec![],
            utterances_count: count,
            total_speech_sec: sec,
        }
    }

    fn utt(start: f64, end: f64, speaker: &str, ch: i32, text: &str) -> Utterance {
        Utterance {
            start,
            end,
            speaker: speaker.to_string(),
            channel: Some(ch),
            text: text.to_string(),
            diarized: true,
        }
    }

    #[test]
    fn merges_same_voice_from_both_channels() {
        let mut infos = vec![
            info("Участник 2", 0, vec![1.0, 0.0, 0.0], 1, 4.0),
            info("Собеседник 1", 1, vec![1.0, 0.0, 0.0], 1, 4.0),
        ];
        let mut utts = vec![
            utt(1.0, 5.0, "Участник 2", 0, "привет как дела"),
            utt(1.2, 5.2, "Собеседник 1", 1, "привет как дела"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "Собеседник 1");
        assert_eq!(utts[0].speaker, "Собеседник 1");
        dedupe_cross_channel(&mut utts);
        assert_eq!(utts.len(), 1);
        assert_eq!(utts[0].channel, Some(1));
    }

    #[test]
    fn merges_same_voice_across_channels_without_overlap() {
        let mut infos = vec![
            info("Участник 1", 0, vec![1.0, 0.0], 1, 3.0),
            info("Собеседник 1", 1, vec![1.0, 0.0], 1, 3.0),
        ];
        let mut utts = vec![
            utt(1.0, 3.0, "Участник 1", 0, "первая реплика"),
            utt(20.0, 22.0, "Собеседник 1", 1, "вторая реплика"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 1);
        assert_eq!(utts[0].speaker, "Собеседник 1");
        assert_eq!(utts[1].speaker, "Собеседник 1");
    }

    #[test]
    fn merges_short_echo_even_with_weak_embedding() {
        let mut infos = vec![
            info("Участник 5", 0, vec![1.0, 0.0], 1, 1.8),
            info("Собеседник 1", 1, vec![0.3, 0.95], 1, 3.0),
        ];
        let mut utts = vec![
            utt(10.0, 11.8, "Участник 5", 0, "Страничку."),
            utt(9.8, 12.8, "Собеседник 1", 1, "Страничку."),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 1);
        assert_eq!(utts[0].speaker, "Собеседник 1");
        assert_eq!(utts[1].speaker, "Собеседник 1");
    }

    #[test]
    fn different_matched_profiles_are_never_merged() {
        let mut a = info("Иван", 0, vec![1.0, 0.0], 1, 5.0);
        a.matched = true;
        a.matched_name = Some("Иван".to_string());
        let mut b = info("Пётр", 1, vec![1.0, 0.0], 1, 5.0);
        b.matched = true;
        b.matched_name = Some("Пётр".to_string());
        let mut infos = vec![a, b];
        let mut utts = vec![
            utt(1.0, 6.0, "Иван", 0, "одна и та же фраза"),
            utt(1.0, 6.0, "Пётр", 1, "одна и та же фраза"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 2);
    }

    #[test]
    fn different_voices_are_not_merged() {
        let mut infos = vec![
            info("Я", 0, vec![1.0, 0.0], 1, 5.0),
            info("Собеседник 1", 1, vec![0.0, 1.0], 1, 5.0),
        ];
        let mut utts = vec![
            utt(1.0, 6.0, "Я", 0, "сегодня хорошая погода"),
            utt(1.0, 6.0, "Собеседник 1", 1, "совсем другой разговор"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 2);
    }

    #[test]
    fn overlapping_same_speaker_keeps_best_copy_not_concat() {
        let utts = vec![
            utt(10.0, 20.0, "Собеседник 1", 0, "один два три четыре"),
            utt(10.2, 20.2, "Собеседник 1", 1, "один два три четыре пять"),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "один два три четыре пять");
    }

    #[test]
    fn non_overlapping_same_speaker_is_joined() {
        let utts = vec![
            utt(1.0, 2.0, "Я", 0, "первая фраза"),
            utt(2.4, 3.0, "Я", 0, "вторая фраза"),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].text.contains("первая"));
        assert!(merged[0].text.contains("вторая"));
    }

    #[test]
    fn speaker_stats_follow_final_utterances() {
        let mut infos = vec![
            info("Я", 0, vec![1.0, 0.0], 9, 99.0),
            info("Пропавший", 0, vec![0.0, 1.0], 9, 99.0),
        ];
        let utts = vec![utt(1.0, 3.0, "Я", 0, "раз"), utt(3.5, 4.0, "Я", 0, "два")];
        refresh_speaker_stats(&mut infos, &utts);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].utterances_count, 2);
        assert_eq!(infos[0].total_speech_sec, 2.5);
    }

    #[test]
    fn short_word_is_not_lost_as_substring() {
        let utts = vec![utt(1.0, 2.0, "Я", 0, "да"), utt(2.2, 3.0, "Я", 0, "давай")];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "да давай");
    }

    /// Тихие короткие фрагменты склеиваются, громкая речь — нет.
    #[test]
    fn merge_quiet_short_keeps_loud_separate() {
        let mut samples = vec![0.005f32; 16 * 16000];
        for x in samples[6 * 16000..7 * 16000].iter_mut() {
            *x = 0.1;
        }
        let groups = vec![
            (0.0, 1.0),
            (2.0, 3.0),
            (4.0, 5.0),
            (6.0, 7.0),
            (8.0, 9.0),
            (10.0, 13.0),
        ];
        let out = merge_quiet_short(&samples, groups);
        assert_eq!(out, vec![(0.0, 5.0), (6.0, 7.0), (8.0, 9.0), (10.0, 13.0)]);
    }

    /// Склейка не растягивает одну реплику больше лимита.
    #[test]
    fn merge_quiet_short_caps_merged_duration() {
        let samples = vec![0.005f32; 16 * 16000];
        let groups: Vec<(f32, f32)> = (0..8)
            .map(|i| (i as f32 * 2.0, i as f32 * 2.0 + 1.0))
            .collect();
        let out = merge_quiet_short(&samples, groups);
        assert_eq!(out.len(), 2);
        assert!(out[0].1 - out[0].0 <= 12.0);
    }

    /// Регрессия: раньше правая граница среза считалась от остатка
    /// (`words.len() - lo`) и паниковала при lo > words.len()/2.
    #[test]
    fn split_words_no_panic_on_late_bounds() {
        let words: Vec<&str> = (0..12).map(|_| "w").collect();
        let bounds = vec![0.0, 1.0, 2.0, 3.0, 20.0];
        let texts = split_words_by_bounds(&words, &bounds);
        assert_eq!(texts.len(), 4);
        assert_eq!(
            texts
                .iter()
                .map(|t| t.split_whitespace().count())
                .sum::<usize>(),
            words.len()
        );
    }

    #[test]
    fn split_words_all_words_distributed() {
        let words = vec!["a", "b", "c", "d", "e"];
        let texts = split_words_by_bounds(&words, &[0.0, 2.0, 4.0]);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts.join(" "), "a b c d e");
    }

    #[test]
    fn profile_loader_skips_foreign_embedding_space_and_uses_prototypes() {
        let dir = std::env::temp_dir().join(format!("mr-vp-space-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("voiceprints.json");
        let emb = vec![0.1f64; mr_ml::EMBEDDING_DIM];
        std::fs::write(
            &file,
            serde_json::to_string(&serde_json::json!({
                "version": 3,
                "embedding_space": "another-space",
                "profiles": [{"name": "Чужой", "embedding": emb.clone()}],
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(read_profiles_file(&file).is_empty());

        std::fs::write(
            &file,
            serde_json::to_string(&serde_json::json!({
                "version": 3,
                "embedding_space": mr_ml::EMBEDDING_SPACE,
                "profiles": [{
                    "name": "Свой",
                    "prototypes": [{
                        "id": "p1", "embedding": emb, "duration_sec": 4.0,
                        "quality": 1.0, "created_at": "", "snippet_relpath": null
                    }]
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        let got = read_profiles_file(&file);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].embedding.len(), mr_ml::EMBEDDING_DIM);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_words_few_words_no_panic() {
        let words = vec!["one"];
        let texts = split_words_by_bounds(&words, &[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(texts.len(), 3);
        assert_eq!(texts[0], "one");
        assert_eq!(texts[1], "");
        assert_eq!(texts[2], "");
    }
}
