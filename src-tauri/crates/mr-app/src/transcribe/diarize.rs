//! Диаризация: субсегментация, кластеризация канала
//! и сопоставление спикеров с профилями Voice Packs.

use std::collections::{HashMap, HashSet};

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;
use mr_ml::{interval_gap, match_voiceprints_matrix, SpeakerProfile};
use mr_packs as packs;

use super::{Sample, SpeakerInfo, Utterance};

/// Группировка VAD-сегментов в окна для ASR (≤ `max_dur`, склейка по паузе ≤ `gap`).
/// Длинную речь режем по минимуму вероятности VAD во второй половине окна.
pub(crate) fn group_segments(
    segments: &[(f32, f32)],
    probs: &[f32],
    max_dur: f32,
    gap: f32,
) -> Vec<(f32, f32)> {
    let win_sec = mr_ml::onnx::vad::VAD_WINDOW as f32 / 16000.0;
    let mut norm: Vec<(f32, f32)> = Vec::new();
    for &(s, e) in segments {
        if e - s <= max_dur {
            norm.push((s, e));
            continue;
        }
        if probs.is_empty() {
            let n = ((e - s) / max_dur).ceil() as usize;
            let step = (e - s) / n as f32;
            for i in 0..n {
                norm.push((s + i as f32 * step, s + (i + 1) as f32 * step));
            }
            continue;
        }
        let mut a = s;
        while e - a > max_dur {
            let window_end = a + max_dur;
            let mid = a + max_dur / 2.0;
            let lo = (mid / win_sec).ceil().max(0.0) as usize;
            let hi = ((window_end / win_sec).ceil().max(0.0) as usize).min(probs.len());
            let cut = (lo..hi)
                .min_by(|&i, &j| probs[i].total_cmp(&probs[j]))
                .map(|i| i as f32 * win_sec)
                .filter(|&cut| cut > a && cut <= window_end)
                .unwrap_or(window_end);
            norm.push((a, cut));
            a = cut;
        }
        norm.push((a, e));
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
pub(crate) fn read_profiles_file(file: &std::path::Path) -> Vec<SpeakerProfile> {
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
///
/// Снижено с 1.4 до 0.6 с (P10): в реальных диалогах второй спикер нередко отвечает
/// только короткими репликами («Ну да», «Хорошо», 0.6–1.0 с). При прежнем пороге 1.4 с
/// в core-пуле оставался единственный доминирующий спикер, а второй участник полностью
/// терялся, принудительно привязываясь к чужому центроиду.
///
/// Замеры ERes2NetV2 (тест `onnx_real::eres2netv2_parity_with_official_reference`):
/// на окнах 0.6 с сходство со своим голосом составляет 0.41–0.79, с чужим — 0.04–0.19
/// (дельта > 0.22), что обеспечивает устойчивое разделение спикеров в кластеризации.
const CORE_MIN_DUR: f64 = 0.6;

/// Кластеры короче 2% речи — обычно шумные короткие эмбеддинги. Клеим их
/// к ближайшему крупному голосу (косинус ≥ 0.30), а непохожие оставляем:
/// это может быть третий человек, сказавший пару слов.
fn merge_tiny_clusters(vecs: &[Vec<f64>], labels: &mut [usize], weights: &[f64], min_keep: usize) {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return;
    }
    let min_sec = (total * 0.02).clamp(2.0, 10.0);
    let mut protected: HashSet<usize> = HashSet::new();
    loop {
        let mut totals: HashMap<usize, f64> = HashMap::new();
        for (i, l) in labels.iter().enumerate() {
            *totals.entry(*l).or_default() += weights[i];
        }
        if totals.len() <= min_keep.max(1) {
            return;
        }
        let Some((&small, _)) = totals
            .iter()
            .filter(|(l, &t)| t < min_sec && !protected.contains(l))
            .min_by(|a, b| a.1.total_cmp(b.1))
        else {
            return;
        };
        let centroid = |l: usize| {
            let members: Vec<usize> = labels
                .iter()
                .enumerate()
                .filter(|(_, &x)| x == l)
                .map(|(i, _)| i)
                .collect();
            let vs: Vec<Vec<f64>> = members.iter().map(|&i| vecs[i].clone()).collect();
            let ws: Vec<f64> = members.iter().map(|&i| weights[i]).collect();
            mr_ml::weighted_centroid(&vs, &ws)
        };
        let cs = centroid(small);
        let mut best = (0usize, -2.0f64);
        for &l in totals.keys() {
            if l == small {
                continue;
            }
            let sim = mr_ml::cosine_similarity(&cs, &centroid(l));
            if sim > best.1 {
                best = (l, sim);
            }
        }
        if best.1 < 0.30 {
            protected.insert(small);
            continue;
        }
        for l in labels.iter_mut() {
            if *l == small {
                *l = best.0;
            }
        }
    }
}

/// Сглаживание меток по времени (только моно): одиночный шумный эмбеддинг
/// не должен переключать спикера, если соседние куски уверенно относятся
/// к другому голосу. Витерби по сходству с центроидами + штраф за смену.
fn smooth_labels_mono(
    utterances: &[Utterance],
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
    quals: &[f64],
    label_of: &mut HashMap<usize, usize>,
) {
    const SWITCH_PENALTY: f64 = 0.15;
    let mut order: Vec<usize> = (0..utterances.len()).collect();
    order.sort_by(|&a, &b| utterances[a].start.total_cmp(&utterances[b].start));
    for _ in 0..2 {
        let mut ids: Vec<usize> = label_of.values().copied().collect();
        ids.sort();
        ids.dedup();
        if ids.len() < 2 {
            return;
        }
        let centroids: Vec<Vec<f64>> = ids
            .iter()
            .map(|&l| {
                let members: Vec<usize> = label_of
                    .iter()
                    .filter(|(_, &x)| x == l)
                    .map(|(&i, _)| i)
                    .filter(|&i| embs[i].is_some())
                    .collect();
                let vs: Vec<Vec<f64>> = members.iter().map(|&i| embs[i].clone().unwrap()).collect();
                let ws: Vec<f64> = members
                    .iter()
                    .map(|&i| durs[i].min(8.0) * quals[i])
                    .collect();
                if vs.is_empty() {
                    vec![]
                } else {
                    mr_ml::weighted_centroid(&vs, &ws)
                }
            })
            .collect();
        let mut back: Vec<Vec<usize>> = Vec::with_capacity(order.len());
        let mut prev = vec![0.0f64; ids.len()];
        for (pos, &i) in order.iter().enumerate() {
            let em: Vec<f64> = match &embs[i] {
                Some(e) => centroids
                    .iter()
                    .map(|c| mr_ml::cosine_similarity(e, c))
                    .collect(),
                None => vec![0.0; ids.len()],
            };
            if pos == 0 {
                prev = em;
                back.push(vec![0; ids.len()]);
                continue;
            }
            let (mut b1, mut b1s, mut b2, mut b2s) =
                (f64::NEG_INFINITY, 0usize, f64::NEG_INFINITY, 0usize);
            for (s, &v) in prev.iter().enumerate() {
                if v > b1 {
                    b2 = b1;
                    b2s = b1s;
                    b1 = v;
                    b1s = s;
                } else if v > b2 {
                    b2 = v;
                    b2s = s;
                }
            }
            let mut cur = vec![0.0f64; ids.len()];
            let mut bp = vec![0usize; ids.len()];
            for s in 0..ids.len() {
                let (other, other_s) = if s == b1s { (b2, b2s) } else { (b1, b1s) };
                if prev[s] >= other - SWITCH_PENALTY {
                    cur[s] = prev[s] + em[s];
                    bp[s] = s;
                } else {
                    cur[s] = other + em[s] - SWITCH_PENALTY;
                    bp[s] = other_s;
                }
            }
            prev = cur;
            back.push(bp);
        }
        let mut state = (0..ids.len())
            .max_by(|&a, &b| prev[a].total_cmp(&prev[b]))
            .unwrap_or(0);
        for pos in (0..order.len()).rev() {
            label_of.insert(order[pos], ids[state]);
            state = back[pos][state];
        }
    }
}

fn rms_quality(chunk: &[f32]) -> f64 {
    if chunk.is_empty() {
        return 0.5;
    }
    let rms = (chunk.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / chunk.len() as f64).sqrt();
    (rms / 0.05).clamp(0.3, 1.5)
}

/// Диаризация одного канала: чистые от перекрытий эмбеддинги → core-кластеризация
/// → short/micro-привязка → ограниченное назначение → профили Voice Packs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn diarize_channel(
    samples: &[f32],
    utterances: &mut [Utterance],
    channel_idx: Option<i32>,
    default_name: &str,
    prefix: &str,
    extractor: &mut Option<EmbeddingExtractor>,
    segmentation: Option<&Segmentation>,
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
                let mut chunk = samples[lo..hi].to_vec();
                let clean_len = segmentation
                    .map(|seg| seg.exclude_overlap(&mut chunk, lo))
                    .unwrap_or(chunk.len());
                if clean_len >= (sr * 0.4) as usize {
                    if let Ok(Some(e)) = ext.embed(&chunk) {
                        quals[i] = rms_quality(&chunk);
                        embs[i] = Some(e);
                    }
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
            silhouette: 0.0,
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
    // Калибровка Complete Linkage для ERes2NetV2: кросс-спикерные пары ≤ 0.22 (dist ≥ 0.78),
    // один голос ≥ 0.48 (dist ≤ 0.52). Порог 0.52 предотвращает расщепление одного человека.
    let mut core_labels = agglomerative(&core_vecs, 0.52, k);
    // Короткие шумные куски часто образуют ложные «голоса». Сливаем их с
    // ближайшим похожим голосом (косинус ≥ 0.30) как в моно, так и в стерео.
    if k.is_none() {
        let weights: Vec<f64> = core.iter().map(|&i| durs[i]).collect();
        merge_tiny_clusters(&core_vecs, &mut core_labels, &weights, min_speakers.max(1));
    }
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
    // Одиночный шумный кусок не должен рвать монолог на два голоса.
    // При явно заданном минимуме спикеров не сглаживаем: метки важнее.
    if channel_idx.is_none() && min_speakers == 0 {
        smooth_labels_mono(utterances, &embs, &durs, &quals, &mut label_of);
    }
    if let Some(segmentation) = segmentation {
        super::speaker_assignment::apply(
            segmentation,
            utterances,
            &embs,
            &durs,
            &quals,
            &mut label_of,
            max_speakers,
        );
    }
    let final_vecs: Vec<Vec<f64>> = valid.iter().map(|&i| embs[i].clone().unwrap()).collect();
    let final_labels: Vec<usize> = valid.iter().map(|&i| label_of[&i]).collect();
    let final_scores = mr_ml::cluster::silhouette_scores(&final_vecs, &final_labels);
    let mut silhouettes = vec![0.0f64; n];
    for (&i, score) in valid.iter().zip(final_scores) {
        silhouettes[i] = score;
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
        let (weighted_sil, sil_weight) =
            members
                .iter()
                .filter(|&&i| embs[i].is_some())
                .fold((0.0, 0.0), |(sum, total), &i| {
                    let weight = durs[i].min(8.0) * quals[i];
                    (sum + silhouettes[i] * weight, total + weight)
                });
        let speaker_silhouette = if sil_weight > 0.0 {
            weighted_sil / sil_weight
        } else {
            0.0
        };
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
            silhouette: (speaker_silhouette * 1000.0).round() / 1000.0,
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
        u.silhouette = silhouettes[i];
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
    // Защита от транзитивного слияния разных распознанных профилей через промежуточные кластеры
    let mut matched_root: HashMap<usize, &str> = HashMap::new();
    for (idx, info) in infos.iter().enumerate() {
        if info.matched {
            matched_root.insert(idx, &info.name);
        }
    }
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = match (&infos[i].embedding, &infos[j].embedding) {
                (Some(a), Some(b)) => mr_ml::cosine_similarity(a, b),
                _ => 0.0,
            };
            let (overlap, echo) = pair_evidence(&by_name, &infos[i].name, &infos[j].name);
            let short = infos[i].utterances_count <= 2 || infos[j].utterances_count <= 2;
            let same_name = infos[i].name == infos[j].name;

            let should_merge =
                if infos[i].channel == infos[j].channel && infos[i].name != infos[j].name {
                    // Внутри одного канала: если один распознан, а второй — безымянный хвост (например, "Я" и профиль пользователя),
                    // либо оба безымянные ("Собеседник 2" и "3") — при уверенном сходстве (≥ 0.52) это один и тот же спикер.
                    // Разные распознанные профили никогда не объединяем.
                    !(infos[i].matched && infos[j].matched) && sim >= 0.52
                } else {
                    // Разные каналы (микрофон и система) или совпадение имени:
                    same_name
                        || (sim >= 0.55 && overlap >= 0.3)
                        || (sim >= 0.58 && !infos[i].matched && !infos[j].matched)
                        || ((infos[i].matched || infos[j].matched) && sim >= 0.50 && overlap >= 0.3)
                        || ((infos[i].matched ^ infos[j].matched) && sim >= 0.55)
                        || ((infos[i].matched ^ infos[j].matched) && echo && overlap >= 0.5)
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
                dsu.union(i, j);
                let nr = dsu.find(i);
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

/// Приклеить текст с учётом общего стыка: ASR мог распознать одни и те же
/// слова в перекрытии кусков. Срезаем самый длинный повторяющийся стык.
fn append_overlap_text(last: &mut String, next: &str) {
    let a: Vec<&str> = last.split_whitespace().collect();
    let b: Vec<&str> = next.split_whitespace().collect();
    let norm = |w: &str| {
        w.trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    };
    let max_k = a.len().min(b.len()).min(10);
    let mut k = 0;
    for cand in (1..=max_k).rev() {
        let same = a[a.len() - cand..]
            .iter()
            .zip(&b[..cand])
            .all(|(x, y)| norm(x) == norm(y));
        if same {
            k = cand;
            break;
        }
    }
    if k < b.len() {
        last.push(' ');
        last.push_str(&b[k..].join(" "));
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
                        append_overlap_text(&mut last.text, &u.text);
                    } else if better_utterance(&u, last) {
                        last.text = u.text.clone();
                        last.channel = u.channel;
                    }
                    last.start = last.start.min(u.start);
                    last.end = last.end.max(u.end);
                    last.silhouette = combined_silhouette;
                    continue;
                }
                if u.start - last.end <= 1.0 {
                    // В одном канале соседние куски — это разные слова,
                    // даже если второй целиком повторяет слова первого.
                    // Проверка на вложенность нужна только для эха между
                    // каналами.
                    if u.channel == last.channel || !words_contained(&u.text, &last.text) {
                        last.text = format!("{} {}", last.text, u.text).trim().to_string();
                    }
                    last.end = last.end.max(u.end);
                    last.silhouette = combined_silhouette;
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

/// Разбивает длинные реплики в моно по естественным паузам тишины (RMS по окнам 20 мс,
/// паузы от ~140 мс) на фрагменты от 0.6 до 6 с (до 6.6 с, если паузы нет).
///
/// Зачем это нужно: в моно микрофон пишет обоих людей в один канал. Эмбеддинг
/// 20-секундного куска сплошной речи — это «средний голос» двух людей, и
/// кластеризация сливает весь разговор в одного спикера. Соседние куски
/// одного голоса потом снова склеиваются в [`merge_utterances`].
pub(crate) fn split_at_pauses(samples: &[f32], utterances: &[Utterance]) -> Vec<Utterance> {
    const FRAME_SEC: f64 = 0.02;
    const MIN_UNIT_SEC: f64 = 0.6;
    const PAUSE_MAX_UNIT: f64 = 6.0;
    const MIN_PAUSE_FRAMES: usize = 5; // 5 сглаженных кадров = тишина от ~140 мс в сыром звуке (отсекает смычки согласных 30–100 мс)
    let frame = (FRAME_SEC * 16000.0) as usize;
    let rms: Vec<f64> = samples
        .chunks(frame)
        .map(|c| {
            (c.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / c.len().max(1) as f64).sqrt()
        })
        .collect();
    if rms.len() < 3 {
        return utterances.to_vec();
    }
    // Сглаживание по 5 кадрам (~0.1 с): одиночный тихий отсчёт внутри слова
    // не должен резать фразу.
    let smooth: Vec<f64> = (0..rms.len())
        .map(|i| {
            let lo = i.saturating_sub(2);
            let hi = (i + 3).min(rms.len());
            rms[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
        })
        .collect();
    let mut sorted = rms.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    // Пауза заметно тише обычной речи. Порог — от медианы и шумового пола,
    // а не в абсолютных единицах: тихая и громкая записи режутся одинаково.
    let noise_floor = sorted[sorted.len() / 20];
    let speech_level = sorted[sorted.len() * 3 / 4];
    // Порог ниже обычной речи, но выше шумового пола: иначе на ровном
    // сигнале «паузой» станет сама речь и реплика рассыплется на куски.
    let quiet = (median * 0.35)
        .max(noise_floor * 3.0)
        .min(speech_level * 0.2);
    let mut out = Vec::with_capacity(utterances.len());
    for u in utterances {
        let words: Vec<&str> = u.text.split_whitespace().collect();
        let mut bounds = vec![u.start];
        // Режем в естественных тихих паузах (от 0.6 с, паузы от ~140 мс): короткие реплики-ответы
        // («Ну да», «Хорошо») часто примыкают к чужой речи внутри одного VAD-окна.
        while *bounds.last().unwrap() + MIN_UNIT_SEC * 2.0 < u.end {
            let from = *bounds.last().unwrap();
            let lo = ((from + MIN_UNIT_SEC) / FRAME_SEC) as usize;
            let hi = (((from + PAUSE_MAX_UNIT) / FRAME_SEC) as usize + 1)
                .min(smooth.len())
                .min((u.end / FRAME_SEC) as usize);
            if lo >= hi {
                break;
            }
            // Ищем первую тихую паузу в окне (минимум внутри впадины от ~140 мс).
            let mut quiet_start = None;
            let mut cut = None;
            for i in lo..hi {
                if smooth[i] <= quiet {
                    if quiet_start.is_none() {
                        quiet_start = Some(i);
                    }
                } else if let Some(qs) = quiet_start {
                    if i - qs >= MIN_PAUSE_FRAMES {
                        let best_i = (qs..i)
                            .min_by(|&a, &b| smooth[a].total_cmp(&smooth[b]))
                            .unwrap();
                        let cut_time = best_i as f64 * FRAME_SEC;
                        if cut_time + MIN_UNIT_SEC <= u.end && cut_time >= from + MIN_UNIT_SEC {
                            cut = Some(cut_time);
                            break;
                        }
                    }
                    quiet_start = None;
                }
            }
            if cut.is_none() {
                if let Some(qs) = quiet_start {
                    if hi - qs >= MIN_PAUSE_FRAMES {
                        let best_i = (qs..hi)
                            .min_by(|&a, &b| smooth[a].total_cmp(&smooth[b]))
                            .unwrap();
                        let cut_time = best_i as f64 * FRAME_SEC;
                        if cut_time + MIN_UNIT_SEC <= u.end && cut_time >= from + MIN_UNIT_SEC {
                            cut = Some(cut_time);
                        }
                    }
                }
            }
            let cut = match cut {
                Some(c) if c > from + 0.2 => c,
                _ => {
                    if from + PAUSE_MAX_UNIT + MIN_UNIT_SEC <= u.end {
                        from + PAUSE_MAX_UNIT
                    } else {
                        break;
                    }
                }
            };
            if cut <= from + 0.2 {
                break;
            }
            bounds.push(cut);
        }
        bounds.push(u.end);
        let n = bounds.len() - 1;
        if n < 2 || words.len() < n {
            out.push(u.clone());
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
                start: (w[0] * 100.0).round() / 100.0,
                end: (w[1] * 100.0).round() / 100.0,
                speaker: u.speaker.clone(),
                channel: u.channel,
                text,
                diarized: false,
                silhouette: 0.0,
            });
        }
    }
    out
}

/// Потолок числа окон на одну реплику: 8 эмбеддингов на 20-секундный
/// фрагмент вместо 37 при шаге 0.5 с. Разрезаем только длинные реплики:
/// на коротких смена голоса почти всегда совпадает с паузой VAD, а каждый
/// лишний эмбеддинг на ARM стоит сотни миллисекунд.
const SPLIT_MIN_DUR: f64 = 8.0;
const SPLIT_MAX_WINDOWS: usize = 8;
const SPLIT_WINDOW_SEC: f64 = 1.6;

/// Шаг скользящего окна: на коротких репликах 0.5 с, на длинных растягивается
/// так, чтобы уложиться в [`SPLIT_MAX_WINDOWS`].
fn split_step(duration: f64) -> f64 {
    let span = (duration - SPLIT_WINDOW_SEC).max(0.0);
    (span / (SPLIT_MAX_WINDOWS - 1) as f64).max(0.5)
}

/// Разрез длинных реплик при смене голоса (стерео: каналы уже разделены,
/// поэтому режем только совсем длинные монологи). Работает по скользящему
/// окну эмбеддингов 1.6 с; разрез при косинусном сходстве < 0.35 (локальный
/// минимум; один голос даёт ~0.5+, смена диктора ≤ 0.22).
pub(crate) fn split_long_by_speaker_change(
    samples: &[f32],
    utterances: Vec<Utterance>,
    extractor: &mut Option<EmbeddingExtractor>,
    on_event: &dyn Fn(u8, &str),
) -> Vec<Utterance> {
    let Some(ext) = extractor else {
        return utterances;
    };
    let sr = 16000.0;
    let mut out = Vec::new();
    let total = utterances.len();
    for (index, u) in utterances.into_iter().enumerate() {
        if index % 5 == 0 || index + 1 == total {
            on_event(
                80,
                &format!("Ищу смену голоса: реплика {} из {total}", index + 1),
            );
        }
        let dur = u.end - u.start;
        if dur < SPLIT_MIN_DUR {
            out.push(u);
            continue;
        }
        // Шаг растягивается только на длинных репликах; на коротких остаётся
        // 0.5 с, чтобы не размывать границу смены голоса.
        let step = split_step(dur);
        let mut wins: Vec<(f64, Vec<f64>)> = vec![];
        let mut t = u.start;
        while t + SPLIT_WINDOW_SEC <= u.end + 1e-6 {
            let lo = (t * sr) as usize;
            let hi = ((t + SPLIT_WINDOW_SEC).min(u.end) * sr) as usize;
            if hi > lo && hi <= samples.len() {
                if let Ok(Some(e)) = ext.embed(&samples[lo..hi]) {
                    wins.push((t, e));
                }
            }
            t += step;
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
                silhouette: 0.0,
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
            let remaining_segs = bounds.len() - 1 - (k + 1);
            let max_nw = words.len().saturating_sub(pos + remaining_segs).max(1);
            let raw_nw = ((words.len() as f64 * frac).round() as usize).clamp(1, max_nw);
            let mut snapped = raw_nw;
            let target = pos + raw_nw;
            let is_punct = |s: &str| s.ends_with(['.', '!', '?', ',', ';', ':']);
            if target > pos && target <= words.len() && is_punct(words[target - 1]) {
                // Уже на знаке препинания
            } else if target > pos + 1 && is_punct(words[target - 2]) {
                // Предыдущее слово оканчивалось знаком препинания (граница предложения)
                snapped = (raw_nw - 1).clamp(1, max_nw);
            } else if target < words.len() && is_punct(words[target]) && raw_nw < max_nw {
                snapped = (raw_nw + 1).clamp(1, max_nw);
            }
            snapped
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
        dedupe_cross_channel, group_segments, merge_quiet_short, merge_tiny_clusters,
        merge_utterances, read_profiles_file, reconcile_channels, refresh_speaker_stats,
        split_at_pauses, split_words_by_bounds, SpeakerInfo, Utterance,
    };

    fn info(name: &str, ch: i32, emb: Vec<f64>, count: usize, sec: f64) -> SpeakerInfo {
        SpeakerInfo {
            id: format!("spk_{name}"),
            name: name.to_string(),
            channel: Some(ch),
            matched: false,
            matched_name: None,
            similarity: 0.0,
            silhouette: 0.0,
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
            silhouette: 0.5,
        }
    }

    #[test]
    fn long_segments_cut_at_vad_minimum_in_second_half() {
        let mut probs = vec![0.9f32; 188];
        probs[31] = 0.0;
        probs[94] = 0.1;
        let out = group_segments(&[(0.0, 6.0)], &probs, 4.0, 0.5);
        assert_eq!(out.len(), 2);
        assert!((out[0].1 - 94.0 * 0.032).abs() < 1e-5);
        assert_eq!(out[1].0, out[0].1);
        assert!(out.iter().all(|(s, e)| e - s <= 4.0));

        let fallback = group_segments(&[(0.0, 6.0)], &[], 4.0, 0.5);
        assert_eq!(fallback, vec![(0.0, 3.0), (3.0, 6.0)]);
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
    fn same_channel_padding_overlap_keeps_both_texts() {
        let utts = vec![
            utt(10.0, 20.0, "Я", 0, "она пишет ничего всё нормально"),
            utt(19.5, 21.0, "Я", 0, "я никуда не поведу"),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].text.contains("никуда"));
        assert!(merged[0].text.contains("нормально"));

        let utts = vec![
            utt(10.0, 20.0, "Я", 0, "я весь ваш материал в полицию угу"),
            utt(
                19.5,
                21.0,
                "Я",
                0,
                "я весь ваш материал в полицию угу они вас",
            ),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged[0].text, "я весь ваш материал в полицию угу они вас");
    }

    #[test]
    fn same_channel_repeated_words_are_not_dropped() {
        let utts = vec![
            utt(1.0, 2.0, "Я", 0, "ну в общем"),
            utt(2.1, 3.0, "Я", 0, "в общем"),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "ну в общем в общем");
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
    fn pauses_split_long_utterance_without_losing_words() {
        let sr = 16000usize;
        let mut samples = vec![0.1f32; 12 * sr];
        for x in samples[5 * sr..5 * sr + sr / 3].iter_mut() {
            *x = 0.0;
        }
        for x in samples[9 * sr..9 * sr + sr / 3].iter_mut() {
            *x = 0.0;
        }
        let utt = || Utterance {
            start: 0.0,
            end: 12.0,
            speaker: "X".to_string(),
            channel: None,
            text: "один два три четыре пять шесть семь восемь".to_string(),
            diarized: false,
            silhouette: 0.0,
        };
        let out = split_at_pauses(&samples, &[utt()]);
        assert!(out.len() >= 2);
        assert!(out.iter().all(|u| u.end - u.start <= 6.01));
        assert_eq!(
            out.iter()
                .map(|u| u.text.clone())
                .collect::<Vec<_>>()
                .join(" "),
            "один два три четыре пять шесть семь восемь"
        );
        // Тихая копия режется там же: порог паузы не зависит от громкости.
        let quiet: Vec<f32> = samples.iter().map(|x| x * 0.01).collect();
        let out_quiet = split_at_pauses(&quiet, &[utt()]);
        assert_eq!(out.len(), out_quiet.len());
    }

    #[test]
    fn temporal_smoothing_removes_single_utterance_flip() {
        let utts: Vec<Utterance> = (0..5)
            .map(|i| utt(i as f64, i as f64 + 1.0, "Я", 0, "x"))
            .collect();
        let a = vec![1.0, 0.0];
        let noisy = vec![0.9, 0.1];
        let embs = vec![
            Some(a.clone()),
            Some(a.clone()),
            Some(noisy),
            Some(a.clone()),
            Some(a.clone()),
        ];
        let durs = vec![1.0; 5];
        let quals = vec![1.0; 5];
        let mut labels: std::collections::HashMap<usize, usize> =
            [(0, 10), (1, 10), (2, 11), (3, 10), (4, 10)]
                .into_iter()
                .collect();
        super::smooth_labels_mono(&utts, &embs, &durs, &quals, &mut labels);
        assert!(labels.values().all(|&l| l == 10));

        let b = vec![0.0, 1.0];
        let embs = vec![
            Some(a.clone()),
            Some(a),
            Some(b.clone()),
            Some(b.clone()),
            Some(b),
        ];
        let mut labels: std::collections::HashMap<usize, usize> =
            [(0, 0), (1, 0), (2, 1), (3, 1), (4, 1)]
                .into_iter()
                .collect();
        super::smooth_labels_mono(&utts, &embs, &durs, &quals, &mut labels);
        assert_eq!(labels[&0], labels[&1]);
        assert_eq!(labels[&2], labels[&4]);
        assert_ne!(labels[&0], labels[&2]);
    }

    #[test]
    fn tiny_noise_cluster_joins_nearest_voice_but_foreign_stays() {
        let vecs = vec![
            vec![1.0, 0.0],
            vec![1.0, 0.0],
            vec![0.0, 1.0],
            vec![0.0, 1.0],
            vec![0.7, 0.7],
        ];
        let weights = vec![10.0, 10.0, 10.0, 10.0, 1.0];
        let mut labels = vec![0usize, 0, 1, 1, 2];
        merge_tiny_clusters(&vecs, &mut labels, &weights, 1);
        assert!(labels[4] == 0 || labels[4] == 1);
        let mut distinct = vec![0usize, 0, 1, 1, 2];
        let mut vecs2 = vecs.clone();
        vecs2[4] = vec![-1.0, 0.0];
        merge_tiny_clusters(&vecs2, &mut distinct, &weights, 1);
        assert_eq!(distinct[4], 2);
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

    #[test]
    fn split_words_snaps_to_punctuation() {
        // Пропорция со сдвигом должна аккуратно привязываться к точке,
        // не отрывая первое слово следующего предложения.
        let words = vec![
            "Первая",
            "фраза",
            "завершена.",
            "Вторая",
            "фраза",
            "началась",
        ];
        let bounds = vec![0.0, 2.8, 6.0];
        let texts = split_words_by_bounds(&words, &bounds);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "Первая фраза завершена.");
        assert_eq!(texts[1], "Вторая фраза началась");
    }

    #[test]
    fn pauses_isolate_short_dialog_response() {
        let sr = 16000usize;
        let mut samples = vec![0.1f32; 6 * sr];
        // Пауза 1: 3.00 - 3.22 с (220 мс)
        for x in samples[3 * sr..(3 * sr + (sr * 22 / 100))].iter_mut() {
            *x = 0.0;
        }
        // Пауза 2: 3.70 - 3.92 с (220 мс) — короткая реплика 0.70 с (типично для «Ну да», «Да»)
        for x in samples[37 * sr / 10..(37 * sr / 10 + (sr * 22 / 100))].iter_mut() {
            *x = 0.0;
        }
        let u = Utterance {
            start: 0.0,
            end: 6.0,
            speaker: "X".to_string(),
            channel: None,
            text: "Вводная фраза первого спикера. Короткий ответ. Финальная фраза разговора."
                .to_string(),
            diarized: false,
            silhouette: 0.0,
        };
        let out = split_at_pauses(&samples, &[u]);
        assert_eq!(out.len(), 3);
        assert!(out[1].text.contains("Короткий ответ"));
        assert_eq!(
            out.iter()
                .map(|s| s.text.clone())
                .collect::<Vec<_>>()
                .join(" "),
            "Вводная фраза первого спикера. Короткий ответ. Финальная фраза разговора."
        );
    }

    #[test]
    fn continuous_speech_with_micro_closures_is_not_fragmented() {
        let sr = 16000usize;
        let mut samples = vec![0.1f32; (4.5 * sr as f64) as usize];
        // Микросмычки согласных 40 мс (2 кадра) на 1.0 с и 2.5 с — не должны резать
        for x in samples[sr..sr + (sr * 4 / 100)].iter_mut() {
            *x = 0.0;
        }
        for x in samples[25 * sr / 10..25 * sr / 10 + (sr * 4 / 100)].iter_mut() {
            *x = 0.0;
        }
        let u = Utterance {
            start: 0.0,
            end: 4.5,
            speaker: "X".to_string(),
            channel: None,
            text: "Непрерывный поток слов без настоящих пауз в диалоге".to_string(),
            diarized: false,
            silhouette: 0.0,
        };
        let out = split_at_pauses(&samples, &[u]);
        // Без пауз от ~140 мс и длине < 6 с реплика не дробится
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].text,
            "Непрерывный поток слов без настоящих пауз в диалоге"
        );
    }

    #[test]
    fn pause_discrimination_distinguishes_closures_from_dialog_turns() {
        let sr = 16000usize;
        // 1. Пауза 90 мс (смычка согласных / микрозаминка) — НЕ должна резать
        let mut samples_closure = vec![0.1f32; 4 * sr];
        for x in samples_closure[2 * sr..(2 * sr + (sr * 9 / 100))].iter_mut() {
            *x = 0.0;
        }
        let u1 = Utterance {
            start: 0.0,
            end: 4.0,
            speaker: "X".to_string(),
            channel: None,
            text: "Фраза со смычкой внутри слова".to_string(),
            diarized: false,
            silhouette: 0.0,
        };
        let out1 = split_at_pauses(&samples_closure, &[u1]);
        assert_eq!(out1.len(), 1, "90 мс пауза не должна дробить реплику");

        // 2. Пауза 150 мс (реальная межрепликовая диалоговая пауза) — ДОЛЖНА резать
        let mut samples_turn = vec![0.1f32; 4 * sr];
        for x in samples_turn[2 * sr..(2 * sr + (sr * 15 / 100))].iter_mut() {
            *x = 0.0;
        }
        let u2 = Utterance {
            start: 0.0,
            end: 4.0,
            speaker: "X".to_string(),
            channel: None,
            text: "Первая реплика диалога. Ответная реплика собеседника.".to_string(),
            diarized: false,
            silhouette: 0.0,
        };
        let out2 = split_at_pauses(&samples_turn, &[u2]);
        assert_eq!(
            out2.len(),
            2,
            "150 мс пауза должна разделять реплики в диалоге"
        );
    }

    #[test]
    fn pauses_split_long_window_leaving_no_chunks_over_max_duration() {
        let sr = 16000usize;
        let total_sec = 20.0f64;
        let mut samples = vec![0.1f32; (total_sec * sr as f64) as usize];
        // Паузы 220 мс каждую секунду от 1.0 с до 19.0 с
        for sec in 1..20 {
            let start = sec * sr;
            let end = start + (sr * 22 / 100);
            for x in samples[start..end].iter_mut() {
                *x = 0.0;
            }
        }
        let words: Vec<String> = (1..=40).map(|i| format!("слово{i}")).collect();
        let u = Utterance {
            start: 0.0,
            end: total_sec,
            speaker: "X".to_string(),
            channel: None,
            text: words.join(" "),
            diarized: false,
            silhouette: 0.0,
        };
        let out = split_at_pauses(&samples, &[u]);
        assert!(
            out.len() >= 15,
            "длинное окно должно быть нарезано по регулярным паузам: {}",
            out.len()
        );
        for seg in &out {
            let dur = seg.end - seg.start;
            assert!(
                dur <= 6.6,
                "сегмент {}-{} слишком длинный ({dur} с > 6.6 с)",
                seg.start,
                seg.end
            );
        }
        assert_eq!(out.first().unwrap().start, 0.0);
        assert_eq!(out.last().unwrap().end, total_sec);
    }
}
