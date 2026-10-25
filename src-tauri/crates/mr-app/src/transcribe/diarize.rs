//! Диаризация: эхоподавление, субсегментация, кластеризация канала
//! и сопоставление спикеров с профилями Voice Packs.

use std::collections::HashMap;

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
                && last.1 - last.0 <= MAX_MERGED_SEC
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
            }
        })
        .collect()
}

/// Подавление эха колонок: гасит в левом канале участки, повторяющие правый
/// (тише ≥8 дБ, корреляция огибающих ≥0.75, задержка 0–125 мс).
pub(crate) fn suppress_echo(left: &[f32], right: &[f32]) -> Vec<f32> {
    const FRAME: usize = 400; // 25 мс
    const MAX_DELAY: usize = 5; // 125 мс
    if left.len() != right.len() || left.len() < FRAME * 24 {
        return left.to_vec();
    }
    let nf = left.len() / FRAME;
    let env = |v: &[f32]| -> Vec<f32> {
        (0..nf)
            .map(|i| {
                let s = &v[i * FRAME..(i + 1) * FRAME];
                (s.iter().map(|x| x * x).sum::<f32>() / FRAME as f32).sqrt()
            })
            .collect()
    };
    let (el, er) = (env(left), env(right));
    let noise_r = {
        let mut s = er.clone();
        s.sort_by(|a, b| a.total_cmp(b));
        s[(nf as f64 * 0.3) as usize] + 1e-6
    };
    let atten = 10f32.powf(8.0 / 20.0);
    let context = 10usize; // окно ±250 мс, задержка тракта 0..125 мс
    let mut mask = vec![false; nf];
    for i in 0..nf {
        if er[i] < noise_r.max(0.02) * 4.0 || el[i] * atten >= er[i] {
            continue;
        }
        let lo = i.saturating_sub(context);
        let hi = (i + context + 1).min(nf);
        if hi - lo < 4 {
            continue;
        }
        let seg_l = &el[lo..hi];
        let mean_l: f32 = seg_l.iter().sum::<f32>() / seg_l.len() as f32;
        let den_l: f32 = seg_l
            .iter()
            .map(|x| (x - mean_l).powi(2))
            .sum::<f32>()
            .sqrt();
        if den_l < 1e-9 {
            continue;
        }
        // Ищем задержку тракта 0..125 мс с лучшей корреляцией огибающих
        let mut best = 0.0f32;
        for d in 0..=MAX_DELAY {
            if lo + d >= hi {
                break;
            }
            let seg_r: Vec<f32> = (lo..hi)
                .map(|k| er[(k as isize - d as isize).max(lo as isize) as usize])
                .collect();
            let mean_r: f32 = seg_r.iter().sum::<f32>() / seg_r.len() as f32;
            let den_r: f32 = seg_r
                .iter()
                .map(|x| (x - mean_r).powi(2))
                .sum::<f32>()
                .sqrt();
            if den_r < 1e-9 {
                continue;
            }
            let cov: f32 = seg_l
                .iter()
                .zip(seg_r.iter())
                .map(|(a, b)| (a - mean_l) * (b - mean_r))
                .sum();
            best = best.max(cov / (den_l * den_r));
            if best >= 0.75 {
                break;
            }
        }
        if best >= 0.75 {
            mask[i] = true;
        }
    }
    let mut out = left.to_vec();
    let mut i = 0;
    while i < nf {
        if mask[i] {
            let mut j = i;
            while j < nf && mask[j] {
                j += 1;
            }
            if j > i + 3 {
                for f in (i.saturating_sub(1))..((j + 1).min(nf)) {
                    for k in 0..FRAME {
                        out[f * FRAME + k] = 0.0;
                    }
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Профили спикеров из включённых Voice Packs (v2 + legacy-массив).
pub(crate) fn load_profiles() -> Vec<SpeakerProfile> {
    let app = packs::default_app_data_dir();
    let mut by_name: HashMap<String, SpeakerProfile> = HashMap::new();
    for pack_dir in packs::get_active_packs(&app).unwrap_or_default() {
        let f = pack_dir.join("voiceprints.json");
        let text = match std::fs::read_to_string(&f) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let arr = if let Some(a) = v.get("profiles").and_then(|x| x.as_array()) {
            a.clone()
        } else if let Some(a) = v.as_array() {
            a.clone()
        } else {
            continue;
        };
        for p in arr {
            if let Ok(prof) = serde_json::from_value::<SpeakerProfile>(p) {
                if !prof.name.trim().is_empty() && !prof.embedding.is_empty() {
                    by_name.insert(prof.name.to_lowercase(), prof);
                }
            }
        }
    }
    by_name.into_values().collect()
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
        if max_speakers > 1 && uniq.len() > max_speakers {
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
        let used: std::collections::HashSet<String> = cluster_name.values().cloned().collect();
        let mut used = used;
        let default_used = used.contains(default_name);
        for (num, c) in unmatched.into_iter().enumerate() {
            let num = num + 1;
            let name = if num == 1 && !default_used && !generic.contains(&default_name) {
                default_name.to_string()
            } else {
                let mut idx = if num == 1 && (default_used && !generic.contains(&default_name)) {
                    num + 1
                } else {
                    num
                };
                while used.contains(&format!("{prefix} {idx}")) {
                    idx += 1;
                }
                format!("{prefix} {idx}")
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
    use super::{merge_quiet_short, split_words_by_bounds};

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
        assert!(out[0].1 - out[0].0 <= 13.0);
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
    fn split_words_few_words_no_panic() {
        let words = vec!["one"];
        let texts = split_words_by_bounds(&words, &[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(texts.len(), 3);
        assert_eq!(texts[0], "one");
        assert_eq!(texts[1], "");
        assert_eq!(texts[2], "");
    }
}
