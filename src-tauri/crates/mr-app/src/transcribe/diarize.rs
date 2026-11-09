use std::collections::{HashMap, HashSet};

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;
use mr_ml::{interval_gap, match_voiceprints_matrix, SpeakerProfile};
use mr_packs as packs;

use super::{Sample, SpeakerInfo, Utterance, WordConf};

/// Группировка VAD-сегментов в окна для ASR (≤ `max_dur`, склейка по паузе ≤ `gap`).
/// Длинную речь режем по минимуму вероятности VAD во второй половине окна.
pub(crate) fn group_segments(
    segments: &[(f32, f32)],
    probs: &[f32],
    max_dur: f32,
    gap: f32,
    ch: i32,
) -> Vec<(f32, f32)> {
    let win_sec = mr_ml::onnx::vad::VAD_WINDOW as f32 / 16000.0;
    let input_segments = segments.len();
    let mut cuts = 0usize;
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
            cuts += 1;
            log::trace!(
                target: "vad",
                ch = ch,
                at_sec = cut,
                p = probs.get((cut / win_sec) as usize).copied().unwrap_or(0.0),
                window_end_sec = window_end;
                "ev=vad.group.cut"
            );
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
    log::debug!(
        target: "vad",
        ch = ch,
        in_segs = input_segments,
        out_groups = grouped.len(),
        cuts = cuts;
        "ev=vad.group"
    );
    grouped
}

/// Склейка тихих коротких фрагментов через более длинную паузу:
/// по отдельности ASR теряет тихую короткую речь. Громкую и длинную речь не трогаем.
pub(crate) fn merge_quiet_short(
    samples: &[f32],
    groups: Vec<(f32, f32)>,
    ch: i32,
) -> Vec<(f32, f32)> {
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
    let mut merged = 0usize;
    for (s, e) in groups {
        if let Some(last) = out.last_mut() {
            let gap = s - last.1;
            let incoming_short = e - s <= SHORT_SEC;
            let rms_a = rms(last.0, last.1);
            let rms_b = rms(s, e);
            if incoming_short
                && (0.0..=MAX_GAP_SEC).contains(&gap)
                && e - last.0 <= MAX_MERGED_SEC
                && rms_a < QUIET_RMS
                && rms_b < QUIET_RMS
            {
                log::debug!(
                    target: "vad",
                    ch = ch,
                    merged = merged + 1,
                    gap_sec = gap,
                    rms_a = rms_a,
                    rms_b = rms_b;
                    "ev=vad.merge_quiet"
                );
                merged += 1;
                last.1 = e;
                continue;
            }
        }
        out.push((s, e));
    }
    out
}

fn pick_samples(utterances: &[Utterance], max_samples: usize) -> Vec<Sample> {
    let mut scored: Vec<(f64, &Utterance)> = utterances
        .iter()
        .map(|u| {
            let dur = (u.end - u.start).max(0.0);
            let words = u.text.split_whitespace().count() as f64;
            let dur_score = 1.0 - ((dur - 3.8).abs() / 3.8).min(1.0);
            (
                dur_score * 2.0 + (words / 10.0).min(1.0) + u.silhouette.clamp(-0.5, 1.0) * 1.5,
                u,
            )
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
    let path = crate::logging::file_token(file);
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(error) => {
            if file.exists() {
                log::warn!(
                    target: "diar",
                    file = path,
                    reason = "read",
                    error = error.to_string();
                    "ev=diar.profile.load"
                );
            }
            return vec![];
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(error) => {
            log::warn!(
                target: "diar",
                file = path,
                reason = "json",
                error = error.to_string();
                "ev=diar.profile.load"
            );
            return vec![];
        }
    };
    let space = v
        .get("embedding_space")
        .and_then(|x| x.as_str())
        .unwrap_or(mr_ml::EMBEDDING_SPACE);
    if space != mr_ml::EMBEDDING_SPACE {
        log::warn!(
            target: "diar",
            file = path,
            reason = "embedding_space",
            space = space;
            "ev=diar.profile.load"
        );
        return vec![];
    }
    let arr = if let Some(a) = v.get("profiles").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.as_array() {
        a.clone()
    } else {
        log::warn!(
            target: "diar",
            file = path,
            reason = "no_profiles";
            "ev=diar.profile.load"
        );
        return vec![];
    };
    let total = arr.len();
    let mut dropped = 0usize;
    let mut out = vec![];
    for p in arr {
        let Ok(mut prof) = serde_json::from_value::<SpeakerProfile>(p) else {
            dropped += 1;
            continue;
        };
        if prof.name.trim().is_empty() {
            dropped += 1;
            continue;
        }
        prof.prototypes
            .retain(|x| x.embedding.len() == mr_ml::EMBEDDING_DIM);
        // Контракт v3: центроид собирается по актуальным прототипам.
        if !prof.prototypes.is_empty() {
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
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        log::warn!(
            target: "diar",
            file = path,
            reason = "invalid_profiles",
            kept = out.len(),
            dropped = dropped,
            total = total;
            "ev=diar.profile.load"
        );
    }
    out
}

/// Профили спикеров из включённых Voice Packs (v2/v3 + legacy-массив).
/// Корневой `voiceprints.json` читается первым как fallback: активные паки
/// важнее старых локальных данных.
pub(crate) fn load_profiles() -> Vec<SpeakerProfile> {
    let app = packs::default_app_data_dir();
    let mut files: Vec<(std::path::PathBuf, String, String)> = vec![];
    let root_vp = app.join("voiceprints.json");
    if root_vp.is_file() {
        files.push((
            root_vp,
            "default".to_string(),
            "Основной профиль".to_string(),
        ));
    }
    match packs::get_active_packs(&app) {
        Ok(active) => {
            for dir in active {
                let pid = dir
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let pname = match packs::get_pack_manifest(&app, &pid) {
                    Ok(manifest) => manifest.name,
                    Err(error) => {
                        log::warn!(
                            target: "diar",
                            file = pid.clone(),
                            reason = "pack_manifest",
                            error = error.to_string();
                            "ev=diar.profile.load"
                        );
                        pid.clone()
                    }
                };
                files.push((dir.join("voiceprints.json"), pid, pname));
            }
        }
        Err(error) => log::warn!(
            target: "diar",
            reason = "active_packs",
            error = error.to_string();
            "ev=diar.profile.load"
        ),
    }
    let mut by_name: HashMap<String, SpeakerProfile> = HashMap::new();
    for (f, pid, pname) in files {
        for mut prof in read_profiles_file(&f) {
            prof.pack_id = pid.clone();
            prof.pack_name = pname.clone();
            by_name.insert(prof.name.trim().to_lowercase(), prof);
        }
    }
    let mut out: Vec<SpeakerProfile> = by_name.into_values().collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

// Калибровка ERes2NetV2 (onnx_real): на 0.6 с сходство со своим голосом 0.41–0.79.
const CORE_MIN_DUR: f64 = 1.0;

fn short_join_threshold(dur: f64) -> f64 {
    const MIN: f64 = 0.36;
    const SPAN: f64 = 0.12;
    MIN + SPAN * ((dur - 0.4) / 0.8).clamp(0.0, 1.0)
}

/// Кластеры короче 2% речи — обычно шумные короткие эмбеддинги. Клеим их
/// к ближайшему крупному голосу (косинус ≥ 0.45), а непохожие оставляем:
/// это может быть третий человек, сказавший пару слов.
fn merge_tiny_clusters(vecs: &[Vec<f64>], labels: &mut [usize], weights: &[f64], min_keep: usize) {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return;
    }
    let min_sec = (total * 0.02).clamp(2.0, 10.0);
    // Кластер с хорошей обособленностью — отдельный голос, пусть и короткий.
    let scores = mr_ml::cluster::silhouette_scores(vecs, labels);
    let mut silhouettes: HashMap<usize, (f64, f64)> = HashMap::new();
    for (i, &label) in labels.iter().enumerate() {
        let entry = silhouettes.entry(label).or_default();
        entry.0 += scores.get(i).copied().unwrap_or(0.0) * weights[i];
        entry.1 += weights[i];
    }
    let well_separated = |label: usize| {
        silhouettes
            .get(&label)
            .is_some_and(|&(sum, weight)| weight > 0.0 && sum / weight > 0.3)
    };
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
            .filter(|(l, &t)| t < min_sec && !protected.contains(l) && !well_separated(**l))
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
        if best.1 < 0.45 {
            protected.insert(small);
            continue;
        }
        log::debug!(
            target: "diar",
            from = small,
            to = best.0,
            similarity = best.1,
            n = totals.get(&small).copied().unwrap_or(0.0);
            "ev=diar.merge_tiny"
        );
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
            let gap = (utterances[i].start - utterances[order[pos - 1]].end).max(0.0);
            let penalty = switch_penalty(gap);
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
                if prev[s] >= other - penalty {
                    cur[s] = prev[s] + em[s];
                    bp[s] = s;
                } else {
                    cur[s] = other + em[s] - penalty;
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

fn switch_penalty(gap: f64) -> f64 {
    const BASE: f64 = 0.18;
    BASE * (-gap.max(0.0) / 0.8).exp()
}

/// Короткие реплики не должны утаскивать узнанный профиль к «среднему» голосу.
#[cfg(test)]
fn profile_anchors(
    centroids: &HashMap<usize, Vec<f64>>,
    profiles: &[SpeakerProfile],
) -> HashMap<usize, Vec<f64>> {
    if profiles.is_empty() {
        return HashMap::new();
    }
    let mut pairs: Vec<(f64, usize, usize)> = centroids
        .iter()
        .flat_map(|(&label, centroid)| {
            profiles.iter().enumerate().map(move |(index, profile)| {
                (
                    mr_ml::cosine_similarity(centroid, &profile.embedding),
                    label,
                    index,
                )
            })
        })
        .collect();
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut labels = HashSet::new();
    let mut used = HashSet::new();
    let mut anchors = HashMap::new();
    for (similarity, label, index) in pairs {
        if similarity < 0.65 || labels.contains(&label) || used.contains(&index) {
            continue;
        }
        labels.insert(label);
        used.insert(index);
        anchors.insert(label, profiles[index].embedding.clone());
    }
    anchors
}

fn fill_labels_by_time(utterances: &[Utterance], label_of: &mut HashMap<usize, usize>) {
    let mut labeled: Vec<usize> = (0..utterances.len())
        .filter(|&i| label_of.contains_key(&i))
        .collect();
    for i in 0..utterances.len() {
        if label_of.contains_key(&i) || labeled.is_empty() {
            continue;
        }
        let (start, end) = (utterances[i].start, utterances[i].end);
        let mut best = (f64::INFINITY, 0usize);
        for &v in &labeled {
            let gap = interval_gap(start, end, utterances[v].start, utterances[v].end);
            if gap < best.0 {
                best = (gap, label_of[&v]);
            }
        }
        label_of.insert(i, best.1);
        labeled.push(i);
    }
}

fn cannot_link_pairs(
    segmentation: Option<&Segmentation>,
    utterances: &[Utterance],
) -> Vec<(usize, usize)> {
    let Some(segmentation) = segmentation else {
        return vec![];
    };
    const SR: f64 = 16000.0;
    let frame_sec = mr_ml::onnx::segmentation::FRAME_SAMPLES as f64 / SR;
    let mut pairs = HashSet::new();
    for window in &segmentation.windows {
        let window_start = window.start_sample as f64 / SR;
        let window_end = (window.start_sample
            + window.classes.len() * mr_ml::onnx::segmentation::FRAME_SAMPLES)
            as f64
            / SR;
        let mut members: [Vec<usize>; 3] = Default::default();
        for (index, utterance) in utterances.iter().enumerate() {
            let start = utterance.start.max(window_start);
            let end = utterance.end.min(window_end);
            if end <= start {
                continue;
            }
            let from =
                (((start - window_start) / frame_sec).floor() as usize).min(window.classes.len());
            let to = (((end - window_start) / frame_sec).ceil() as usize).min(window.classes.len());
            let mut overlap = [0.0f64; 3];
            for &class in &window.classes[from..to] {
                if (1..=3).contains(&class) {
                    overlap[class as usize - 1] += 1.0;
                }
            }
            let total: f64 = overlap.iter().sum();
            let Some((local, &best)) = overlap.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))
            else {
                continue;
            };
            if total > 0.0 && best / total >= 0.5 {
                members[local].push(index);
            }
        }
        for a in 0..3 {
            for b in (a + 1)..3 {
                for &i in &members[a] {
                    for &j in &members[b] {
                        if i != j {
                            pairs.insert((i.min(j), i.max(j)));
                        }
                    }
                }
            }
        }
    }
    pairs.into_iter().collect()
}

struct AcoustUnit {
    utterance: usize,
    start: f64,
    end: f64,
    dur: f64,
    quality: f64,
    embedding: Vec<f64>,
}

#[allow(clippy::too_many_arguments)]
fn build_acoust_units(
    samples: &[f32],
    utterances: &[Utterance],
    extractor: &mut Option<EmbeddingExtractor>,
    segmentation: Option<&Segmentation>,
    on_event: &dyn Fn(u8, &str),
) -> Vec<AcoustUnit> {
    const SR: f64 = 16000.0;
    const UNIT_SEC: f64 = 1.8;
    const MIN_UNIT_SEC: f64 = 0.4;
    const MAX_UNIT_SEC: f64 = 2.4;
    const MAX_UNITS: usize = 8;
    let Some(ext) = extractor else {
        return vec![];
    };
    let total = utterances.len();
    let mut units = vec![];
    for (index, utterance) in utterances.iter().enumerate() {
        let duration = (utterance.end - utterance.start).max(0.0);
        if duration >= MIN_UNIT_SEC {
            let windows = if duration <= MAX_UNIT_SEC {
                1
            } else {
                (((duration - UNIT_SEC) / 0.9).ceil() as usize + 1).clamp(2, MAX_UNITS)
            };
            let step = if windows <= 1 {
                duration
            } else {
                ((duration - UNIT_SEC) / (windows - 1) as f64).max(0.9)
            };
            for window in 0..windows {
                let start = utterance.start + window as f64 * step;
                let end = (start + UNIT_SEC).min(utterance.end);
                if end - start < MIN_UNIT_SEC {
                    break;
                }
                let lo = (start.max(0.0) * SR) as usize;
                let hi = ((end.max(0.0) * SR) as usize).min(samples.len());
                if hi <= lo {
                    continue;
                }
                let mut chunk = samples[lo..hi].to_vec();
                let clean_len = segmentation
                    .map(|seg| seg.exclude_overlap(&mut chunk, lo))
                    .unwrap_or(chunk.len());
                if clean_len >= (SR * MIN_UNIT_SEC) as usize {
                    let clean = &chunk[..clean_len];
                    let started = std::time::Instant::now();
                    let result = ext.embed(clean);
                    if log::log_enabled!(target: "diar", log::Level::Trace) {
                        let (outcome, dim, norm) = match &result {
                            Ok(Some(embedding)) => (
                                "ok",
                                embedding.len(),
                                embedding.iter().map(|x| x * x).sum::<f64>().sqrt(),
                            ),
                            Ok(None) => ("none", 0, 0.0),
                            Err(_) => ("error", 0, 0.0),
                        };
                        log::trace!(
                            target: "diar",
                            unit = index,
                            dur_sec = clean_len as f64 / SR,
                            dim = dim,
                            norm = norm,
                            result = outcome,
                            elapsed_ms = started.elapsed().as_millis() as u64;
                            "ev=diar.embed"
                        );
                    }
                    if let Ok(Some(embedding)) = result {
                        units.push(AcoustUnit {
                            utterance: index,
                            start,
                            end,
                            dur: clean_len as f64 / SR,
                            quality: rms_quality(clean),
                            embedding,
                        });
                    }
                }
            }
        }
        if index % 20 == 0 || index + 1 == total {
            on_event(
                (80 + 8 * index / total.max(1)) as u8,
                &format!("Определяю спикеров: {index} из {total} реплик"),
            );
        }
    }
    units
}

fn fill_unit_labels_by_time(units: &[AcoustUnit], label_of: &mut HashMap<usize, usize>) {
    let mut labeled: Vec<usize> = label_of.keys().copied().collect();
    if labeled.is_empty() {
        return;
    }
    labeled.sort_by(|&a, &b| units[a].start.total_cmp(&units[b].start));
    for (index, unit) in units.iter().enumerate() {
        if label_of.contains_key(&index) {
            continue;
        }
        let mut best = (f64::INFINITY, 0usize);
        for &other in &labeled {
            let gap = interval_gap(unit.start, unit.end, units[other].start, units[other].end);
            if gap < best.0 {
                best = (gap, other);
            }
        }
        if let Some(&label) = label_of.get(&best.1) {
            label_of.insert(index, label);
            labeled.push(index);
        }
    }
}

fn log_cluster_summary(
    ch: i32,
    k_req: Option<usize>,
    vecs: &[Vec<f64>],
    labels: &[usize],
    cannot_link: usize,
) {
    if !cfg!(debug_assertions) || vecs.is_empty() || labels.is_empty() {
        return;
    }
    let mut ids: Vec<usize> = labels.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let silhouette = if ids.len() > 1 {
        let scores = mr_ml::cluster::silhouette_scores(vecs, labels);
        scores.iter().sum::<f64>() / scores.len() as f64
    } else {
        0.0
    };
    let centroids: Vec<Vec<f64>> = ids
        .iter()
        .filter_map(|&label| {
            let members: Vec<usize> = labels
                .iter()
                .enumerate()
                .filter(|(_, &value)| value == label)
                .map(|(index, _)| index)
                .collect();
            if members.is_empty() {
                return None;
            }
            let points: Vec<Vec<f64>> = members.iter().map(|&i| vecs[i].clone()).collect();
            let weights = vec![1.0; points.len()];
            Some(mr_ml::weighted_centroid(&points, &weights))
        })
        .collect();
    let mut distances = Vec::new();
    for i in 0..centroids.len() {
        for j in (i + 1)..centroids.len() {
            distances.push(1.0 - mr_ml::cosine_similarity(&centroids[i], &centroids[j]));
        }
    }
    let (dist_min, dist_mean, dist_max) = if distances.is_empty() {
        (0.0, 0.0, 0.0)
    } else {
        let min = distances.iter().copied().fold(f64::INFINITY, f64::min);
        let max = distances.iter().copied().fold(0.0f64, f64::max);
        (
            min,
            distances.iter().sum::<f64>() / distances.len() as f64,
            max,
        )
    };
    log::info!(
        target: "diar",
        ch = ch,
        n = labels.len(),
        k_req = k_req.unwrap_or(0),
        k = ids.len(),
        silhouette = silhouette,
        dist_min = dist_min,
        dist_mean = dist_mean,
        dist_max = dist_max,
        cannot_link = cannot_link;
        "ev=diar.cluster"
    );
}

const IMPURE_CLUSTER_MIN_MEMBERS: usize = 3;
// Порог эмпирический: выше него в кластере смешаны голоса и профиль доверять нельзя.
const IMPURE_CLUSTER_MAX_DISTANCE: f64 = 0.35;

fn cluster_is_impure(members: &[&Vec<f64>], centroid: &[f64]) -> bool {
    if members.len() < IMPURE_CLUSTER_MIN_MEMBERS {
        return false;
    }
    let avg_dist = members
        .iter()
        .map(|vector| 1.0 - mr_ml::cosine_similarity(vector, centroid))
        .sum::<f64>()
        / members.len() as f64;
    avg_dist > IMPURE_CLUSTER_MAX_DISTANCE
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn diarize_channel(
    samples: &[f32],
    utterances: &mut Vec<Utterance>,
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
    use mr_ml::{cluster::agglomerative_weighted, cosine_similarity};
    let n = utterances.len();
    if n == 0 {
        return vec![];
    }
    let ch = channel_idx.unwrap_or(-1);
    log::debug!(
        target: "diar",
        ch = ch,
        utts = n,
        embed = extractor.is_some(),
        segmentation = segmentation.is_some(),
        min_spk = min_speakers,
        max_spk = max_speakers;
        "ev=diar.start"
    );
    let mut embs: Vec<Option<Vec<f64>>> = vec![None; n];
    let mut durs: Vec<f64> = utterances
        .iter()
        .map(|u| (u.end - u.start).max(0.0))
        .collect();
    let mut quals = vec![1.0f64; n];
    let units = build_acoust_units(samples, utterances, extractor, segmentation, on_event);
    if units.is_empty() {
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
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: 0.0,
            silhouette: 0.0,
            candidates: vec![],
            embedding: None,
            samples: vec![],
            utterances_count: utterances.len(),
            total_speech_sec: utterances.iter().map(|u| (u.end - u.start).max(0.0)).sum(),
        }];
    }
    let mut core_units: Vec<usize> = (0..units.len())
        .filter(|&index| units[index].dur >= CORE_MIN_DUR)
        .collect();
    if core_units.is_empty() {
        core_units = (0..units.len()).collect();
    }
    let (dur_min, dur_max) = core_units
        .iter()
        .fold((f64::INFINITY, 0.0f64), |(min, max), &index| {
            (min.min(units[index].dur), max.max(units[index].dur))
        });
    log::debug!(
        target: "diar",
        ch = ch,
        utts = n,
        units = units.len(),
        core_units = core_units.len(),
        dur_min = if dur_min.is_finite() { dur_min } else { 0.0 },
        dur_max = dur_max;
        "ev=diar.units"
    );
    let core_vecs: Vec<Vec<f64>> = core_units
        .iter()
        .map(|&index| units[index].embedding.clone())
        .collect();
    let k = if min_speakers > 0 && min_speakers == max_speakers {
        Some(min_speakers)
    } else {
        None
    };
    // Замеры ERes2NetV2 (`onnx_real`): разные дикторы ≤ 0.22 (dist ≥ 0.78),
    // один голос ≥ 0.48 (dist ≤ 0.52). shortcut: порог 0.52 перенесён с
    // complete linkage без замера average; перекалибровать на встречах.
    let core_weights: Vec<f64> = core_units
        .iter()
        .map(|&index| units[index].dur.clamp(0.4, 4.0) * units[index].quality)
        .collect();
    let core_pos: HashMap<usize, usize> = core_units
        .iter()
        .enumerate()
        .map(|(position, &index)| (index, position))
        .collect();
    let mut constraints = vec![];
    if let Some(segmentation) = segmentation {
        let blocked: HashSet<(usize, usize)> = cannot_link_pairs(Some(segmentation), utterances)
            .into_iter()
            .collect();
        let mut by_utterance: HashMap<usize, Vec<usize>> = HashMap::new();
        for &index in &core_units {
            by_utterance
                .entry(units[index].utterance)
                .or_default()
                .push(core_pos[&index]);
        }
        for (a, b) in blocked {
            let (Some(left), Some(right)) = (by_utterance.get(&a), by_utterance.get(&b)) else {
                continue;
            };
            for &pa in left {
                for &pb in right {
                    constraints.push((pa.min(pb), pa.max(pb)));
                }
            }
        }
    }
    let mut core_labels = agglomerative_weighted(&core_vecs, &core_weights, &constraints, 0.52, k);
    // Короткие шумные куски часто образуют ложные «голоса». Сливаем их с
    // ближайшим похожим голосом (косинус ≥ 0.45) как в моно, так и в стерео.
    if k.is_none() {
        let weights: Vec<f64> = core_units.iter().map(|&index| units[index].dur).collect();
        merge_tiny_clusters(&core_vecs, &mut core_labels, &weights, min_speakers.max(1));
    }
    if k.is_none() {
        let mut uniq: Vec<usize> = core_labels.clone();
        uniq.sort();
        uniq.dedup();
        if max_speakers > 0 && uniq.len() > max_speakers {
            log::debug!(
                target: "diar",
                ch = ch,
                before = uniq.len(),
                after = max_speakers,
                max_spk = max_speakers;
                "ev=diar.max_speakers"
            );
            core_labels = agglomerative_weighted(
                &core_vecs,
                &core_weights,
                &constraints,
                f64::INFINITY,
                Some(max_speakers),
            );
        } else if min_speakers > 1 && uniq.len() < min_speakers && core_vecs.len() >= min_speakers {
            log::debug!(
                target: "diar",
                ch = ch,
                before = uniq.len(),
                after = min_speakers,
                min_spk = min_speakers;
                "ev=diar.max_speakers"
            );
            core_labels = agglomerative_weighted(
                &core_vecs,
                &core_weights,
                &constraints,
                f64::INFINITY,
                Some(min_speakers),
            );
        }
    }
    log_cluster_summary(ch, k, &core_vecs, &core_labels, constraints.len());
    on_event(88, "Голоса разделены по спикерам");
    let mut unit_label: HashMap<usize, usize> = core_units
        .iter()
        .copied()
        .zip(core_labels.iter().copied())
        .collect();
    let recompute_units = |label_of: &HashMap<usize, usize>| -> HashMap<usize, Vec<f64>> {
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for (&index, &label) in label_of {
            groups.entry(label).or_default().push(index);
        }
        groups
            .into_iter()
            .map(|(label, members)| {
                let vecs: Vec<Vec<f64>> = members
                    .iter()
                    .map(|&index| units[index].embedding.clone())
                    .collect();
                let ws: Vec<f64> = members
                    .iter()
                    .map(|&index| units[index].dur.clamp(0.4, 4.0) * units[index].quality)
                    .collect();
                (label, mr_ml::weighted_centroid(&vecs, &ws))
            })
            .collect()
    };
    let mut unit_centroids = recompute_units(&unit_label);
    for (index, unit) in units.iter().enumerate() {
        if unit_label.contains_key(&index) {
            continue;
        }
        let mut best = (f64::NEG_INFINITY, 0usize);
        let mut second = (f64::NEG_INFINITY, 0usize);
        for (&label, centroid) in unit_centroids.iter() {
            let score = cosine_similarity(&unit.embedding, centroid);
            if score > best.0 {
                second = best;
                best = (score, label);
            } else if score > second.0 {
                second = (score, label);
            }
        }
        let threshold = short_join_threshold(unit.dur);
        log::debug!(
            target: "diar",
            ch = ch,
            i = index,
            top1_id = best.1 as i64,
            top1_sim = best.0,
            top2_id = second.1 as i64,
            top2_sim = if second.0.is_finite() { second.0 } else { 0.0 },
            margin = if second.0.is_finite() { best.0 - second.0 } else { best.0 },
            threshold = threshold,
            matched = best.0 >= threshold;
            "ev=diar.assign"
        );
        if best.0 >= threshold {
            unit_label.insert(index, best.1);
            unit_centroids = recompute_units(&unit_label);
        }
    }
    fill_unit_labels_by_time(&units, &mut unit_label);
    let mut label_duration: HashMap<(usize, usize), f64> = HashMap::new();
    for (index, unit) in units.iter().enumerate() {
        if let Some(&label) = unit_label.get(&index) {
            *label_duration.entry((unit.utterance, label)).or_default() += unit.dur;
        }
    }
    let mut label_of: HashMap<usize, usize> = HashMap::new();
    let mut valid: Vec<usize> = vec![];
    for utterance in 0..n {
        let Some((&(_, label), _)) = label_duration
            .iter()
            .filter(|((index, _), _)| *index == utterance)
            .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0 .1.cmp(&a.0 .1)))
        else {
            continue;
        };
        let members: Vec<usize> = (0..units.len())
            .filter(|&index| {
                units[index].utterance == utterance && unit_label.get(&index) == Some(&label)
            })
            .collect();
        if members.is_empty() {
            continue;
        }
        let vecs: Vec<Vec<f64>> = members
            .iter()
            .map(|&index| units[index].embedding.clone())
            .collect();
        let ws: Vec<f64> = members
            .iter()
            .map(|&index| units[index].dur.clamp(0.4, 4.0) * units[index].quality)
            .collect();
        let total_dur: f64 = members.iter().map(|&index| units[index].dur).sum();
        let quality: f64 = members
            .iter()
            .map(|&index| units[index].quality * units[index].dur)
            .sum::<f64>()
            / total_dur.max(1e-6);
        embs[utterance] = Some(mr_ml::weighted_centroid(&vecs, &ws));
        durs[utterance] = (utterances[utterance].end - utterances[utterance].start).max(0.0);
        quals[utterance] = quality;
        label_of.insert(utterance, label);
        valid.push(utterance);
    }
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
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: 0.0,
            silhouette: 0.0,
            candidates: vec![],
            embedding: None,
            samples: vec![],
            utterances_count: utterances.len(),
            total_speech_sec: utterances.iter().map(|u| (u.end - u.start).max(0.0)).sum(),
        }];
    }
    fill_labels_by_time(utterances, &mut label_of);
    // Одиночный шумный кусок не должен рвать монолог на два голоса.
    // При явно заданном минимуме спикеров не сглаживаем: метки важнее.
    if channel_idx.is_none() && min_speakers == 0 {
        smooth_labels_mono(utterances, &embs, &durs, &quals, &mut label_of);
    }
    let mut spans = Vec::new();
    if let Some(segmentation) = segmentation {
        spans = super::speaker_assignment::apply(
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
    let mut raw_clusters: Vec<usize> = label_of.values().copied().collect();
    raw_clusters.sort();
    raw_clusters.dedup();
    let ordered_centroids: Vec<Vec<f64>> = raw_clusters
        .iter()
        .map(|c| {
            let members: Vec<usize> = {
                let mut members: Vec<usize> = label_of
                    .iter()
                    .filter(|(_, &l)| l == *c)
                    .map(|(&i, _)| i)
                    .collect();
                // Порядок обхода HashMap не должен влиять на сумму центроида.
                members.sort_unstable();
                members
            };
            // Пары «вектор-вес» строим вместе: иначе веса съезжают на
            // реплики без эмбеддинга и центроид считается не по тем голосам.
            let weighted: Vec<(Vec<f64>, f64)> = members
                .iter()
                .filter_map(|&i| {
                    embs[i]
                        .clone()
                        .map(|vector| (vector, durs[i].min(8.0) * quals[i]))
                })
                .collect();
            if weighted.is_empty() {
                vec![0.0; mr_ml::EMBEDDING_DIM]
            } else {
                let (vecs, ws): (Vec<Vec<f64>>, Vec<f64>) = weighted.into_iter().unzip();
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
    let mut cluster_name: HashMap<usize, String> = HashMap::new();
    let mut cluster_pack_id: HashMap<usize, String> = HashMap::new();
    let mut cluster_pack_name: HashMap<usize, String> = HashMap::new();
    let mut unmatched = vec![];
    for (k, &c) in raw_clusters.iter().enumerate() {
        if matches[k].matched {
            let m_name = matches[k].name.clone().unwrap();
            let members: Vec<usize> = label_of
                .iter()
                .filter(|(_, &l)| l == c)
                .map(|(&i, _)| i)
                .collect();
            let member_embs: Vec<&Vec<f64>> =
                members.iter().filter_map(|&i| embs[i].as_ref()).collect();
            let impure = cluster_is_impure(&member_embs, &ordered_centroids[k]);
            if !impure {
                cluster_name.insert(c, m_name);
                if let Some(pid) = &matches[k].pack_id {
                    cluster_pack_id.insert(c, pid.clone());
                }
                if let Some(pname) = &matches[k].pack_name {
                    cluster_pack_name.insert(c, pname.clone());
                }
            } else {
                log::info!(
                    target: "diar",
                    cluster = c,
                    profile = crate::logging::speaker_token(&m_name),
                    members = member_embs.len();
                    "ev=diar.voiceprint.rejected_impure_cluster"
                );
                unmatched.push(c);
            }
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
        let members: Vec<usize> = {
            let mut members: Vec<usize> = label_of
                .iter()
                .filter(|(_, &l)| cids.contains(&l))
                .map(|(&i, _)| i)
                .collect();
            members.sort_unstable();
            members
        };
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
            let weighted: Vec<(Vec<f64>, f64)> = members
                .iter()
                .filter_map(|&i| {
                    embs[i]
                        .clone()
                        .map(|vector| (vector, durs[i].min(8.0) * quals[i]))
                })
                .collect();
            if weighted.is_empty() {
                None
            } else {
                let (vecs, ws): (Vec<Vec<f64>>, Vec<f64>) = weighted.into_iter().unzip();
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
        let matched_pack_id = if is_matched {
            cids.iter().find_map(|c| cluster_pack_id.get(c)).cloned()
        } else {
            None
        };
        let matched_pack_name = if is_matched {
            cids.iter().find_map(|c| cluster_pack_name.get(c)).cloned()
        } else {
            None
        };
        infos.push(SpeakerInfo {
            id: spk_id,
            name: name.clone(),
            channel: channel_idx,
            matched: is_matched,
            matched_name: if is_matched { Some(name.clone()) } else { None },
            matched_pack_id,
            matched_pack_name,
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
    // Кадры PyAnnote уточняют только длинные монологи: на коротких
    let timeline: Vec<Utterance> = spans
        .iter()
        .filter_map(|span| {
            cluster_name.get(&span.label).map(|name| Utterance {
                start: span.start,
                end: span.end,
                speaker: name.clone(),
                channel: channel_idx,
                text: String::new(),
                diarized: true,
                silhouette: 0.0,
                words: Vec::new(),
                ctc_used: false,
            })
        })
        .collect();
    if !timeline.is_empty() {
        utterances.retain(|u| u.end - u.start < 20.0);
        utterances.extend(timeline);
    }
    infos
}

/// ASR-сегмент с padding иногда дослышит начало следующей реплики: слова
/// получают время уже за `end` этого сегмента. Переносим такой хвост в
/// соседнюю реплику, иначе он наследует чужого спикера.
pub(crate) fn move_boundary_words(utterances: &mut Vec<Utterance>) {
    if utterances.len() < 2 {
        return;
    }
    utterances.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut i = 0;
    while i + 1 < utterances.len() {
        if utterances[i].channel != utterances[i + 1].channel
            || utterances[i + 1].start - utterances[i].end > 0.5
        {
            i += 1;
            continue;
        }
        let split = utterances[i]
            .words
            .iter()
            .position(|w| w.start_sec >= utterances[i].end - 0.05)
            .unwrap_or(utterances[i].words.len());
        if split == utterances[i].words.len() || split == 0 {
            i += 1;
            continue;
        }
        let moved = utterances[i].words.split_off(split);
        utterances[i].text = utterances[i]
            .words
            .iter()
            .map(|w| w.w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let moved_text = moved
            .iter()
            .map(|w| w.w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let next = &mut utterances[i + 1];
        next.end = next.end.max(moved.last().map_or(0.0, |w| w.end_sec));
        next.words.splice(0..0, moved);
        next.text = if next.text.is_empty() {
            moved_text
        } else {
            format!("{moved_text} {}", next.text)
        };
        if utterances[i].text.is_empty() {
            utterances.remove(i);
        } else {
            i += 1;
        }
    }
}

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
    // ASR в двух каналах может добавить/потерять служебные слова. Короткие
    // реплики («да», «угу») — это ответы, а не эхо: подмножество даёт буст
    // только от трёх слов, иначе встречная реплика стиралась бы как дубль.
    let subset = wa.is_subset(&wb) || wb.is_subset(&wa);
    if subset && wa.len().min(wb.len()) >= 3 {
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
    let before = utterances.len();
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
                    // В одном канале соседние куски — это разные слова,
                    // даже если второй целиком повторяет слова первого.
                    // Проверка на вложенность нужна только для эха между
                    // каналами.
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
        if n < 2 {
            out.push(u.clone());
            continue;
        }
        let parts = split_utterance_by_bounds(u, &bounds);
        if parts.is_empty() {
            out.push(u.clone());
        } else {
            out.extend(parts);
        }
    }
    out
}

/// Эмбеддинг на ARM стоит сотни миллисекунд: короткие реплики не режем,
/// число окон ограничиваем.
const SPLIT_MIN_DUR: f64 = 3.0;
const SPLIT_MAX_WINDOWS: usize = 8;
const SPLIT_WINDOW_SEC: f64 = 1.6;

/// Шаг скользящего окна: на коротких репликах 0.5 с, на длинных растягивается
/// так, чтобы уложиться в [`SPLIT_MAX_WINDOWS`].
fn split_step(duration: f64) -> f64 {
    let span = (duration - SPLIT_WINDOW_SEC).max(0.0);
    (span / (SPLIT_MAX_WINDOWS - 1) as f64).max(0.5)
}

/// +0.8 с компенсирует отставание окна: эмбеддинг 1.6 с «видит» обоих
/// говорящих ещё какое-то время после стыка.
fn speaker_change_cuts(start: f64, end: f64, wins: &[(f64, Vec<f64>)]) -> Vec<f64> {
    let sims: Vec<f64> = wins
        .windows(2)
        .map(|w| mr_ml::cosine_similarity(&w[0].1, &w[1].1))
        .collect();
    let mut cuts = vec![];
    for (i, &s) in sims.iter().enumerate() {
        if s >= 0.35 {
            continue;
        }
        let prev = if i > 0 { sims[i - 1] } else { 1.0 };
        let next = sims.get(i + 1).copied().unwrap_or(1.0);
        if s > prev || s > next {
            continue;
        }
        let b = ((wins[i].0 + wins[i + 1].0) / 2.0 + 0.8).clamp(start + 0.5, end - 0.5);
        if cuts.last().map(|&c| b - c >= 1.0).unwrap_or(true) {
            cuts.push((b * 100.0).round() / 100.0);
        }
    }
    cuts
}

/// Один голос даёт ~0.5+, смена диктора ≤ 0.22 — замеры ERes2NetV2
/// (`onnx_real::eres2netv2_parity_with_official_reference`).
/// Перекрытия вырезаются до эмбеддинга: иначе оба голоса дают «средний» вектор.
pub(crate) fn split_long_by_speaker_change(
    samples: &[f32],
    utterances: Vec<Utterance>,
    extractor: &mut Option<EmbeddingExtractor>,
    segmentation: Option<&Segmentation>,
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
        let step = split_step(dur);
        let mut wins: Vec<(f64, Vec<f64>)> = vec![];
        let mut t = u.start;
        while t + SPLIT_WINDOW_SEC <= u.end + 1e-6 {
            let lo = (t * sr) as usize;
            let hi = ((t + SPLIT_WINDOW_SEC).min(u.end) * sr) as usize;
            if hi > lo && hi <= samples.len() {
                let mut chunk = samples[lo..hi].to_vec();
                let clean_len = segmentation
                    .map(|seg| seg.exclude_overlap(&mut chunk, lo))
                    .unwrap_or(chunk.len());
                if clean_len >= (sr * 0.4) as usize {
                    if let Ok(Some(e)) = ext.embed(&chunk[..clean_len]) {
                        wins.push((t, e));
                    }
                }
            }
            t += step;
        }
        if wins.len() < 3 {
            log::debug!(
                target: "diar",
                i = index,
                dur_sec = dur,
                windows = wins.len(),
                cuts = 0,
                reason = "too_few_windows";
                "ev=diar.split"
            );
            out.push(u);
            continue;
        }
        let cuts = speaker_change_cuts(u.start, u.end, &wins);
        log::debug!(
            target: "diar",
            i = index,
            dur_sec = dur,
            windows = wins.len(),
            cuts = cuts.len(),
            reason = if cuts.is_empty() { "no_change" } else { "split" };
            "ev=diar.split"
        );
        if cuts.is_empty() {
            out.push(u);
            continue;
        }
        let mut bounds = vec![u.start];
        bounds.extend(cuts);
        bounds.push(u.end);
        let parts = split_utterance_by_bounds(&u, &bounds);
        if parts.is_empty() {
            out.push(u);
        } else {
            out.extend(parts);
        }
    }
    out
}

pub(crate) fn split_utterance_by_bounds(u: &Utterance, bounds: &[f64]) -> Vec<Utterance> {
    if bounds.len() < 2 {
        return vec![u.clone()];
    }
    let words: Vec<&str> = u.text.split_whitespace().collect();
    if words.is_empty() {
        return vec![u.clone()];
    }

    let has_timed_words = !u.words.is_empty() && u.words.iter().any(|w| w.end_sec > w.start_sec);

    if has_timed_words {
        let word_bounds =
            super::word_speakers::resolve_word_bounds(&words, &u.words, u.start, u.end);
        let n_segs = bounds.len() - 1;
        let mut seg_words: Vec<Vec<WordConf>> = vec![Vec::new(); n_segs];
        for (i, &w_text) in words.iter().enumerate() {
            let (s, e) = word_bounds[i];
            let mid = (s + e) / 2.0;
            let mut seg_idx = n_segs - 1;
            for (idx, w) in bounds.windows(2).enumerate() {
                if mid < w[1] || idx == n_segs - 1 {
                    seg_idx = idx;
                    break;
                }
            }
            let wc = if let Some(mut w) = u.words.get(i).cloned() {
                if w.end_sec <= w.start_sec {
                    w.start_sec = (s * 1000.0).round() / 1000.0;
                    w.end_sec = (e * 1000.0).round() / 1000.0;
                }
                w
            } else {
                WordConf {
                    w: w_text.to_string(),
                    conf: 1.0,
                    needs_review: false,
                    start_sec: (s * 1000.0).round() / 1000.0,
                    end_sec: (e * 1000.0).round() / 1000.0,
                }
            };
            seg_words[seg_idx].push(wc);
        }

        let mut parts = Vec::new();
        for (idx, w) in bounds.windows(2).enumerate() {
            let words = &seg_words[idx];
            if words.is_empty() {
                continue;
            }
            let text = words
                .iter()
                .map(|w| w.w.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            parts.push(Utterance {
                start: (w[0] * 100.0).round() / 100.0,
                end: (w[1] * 100.0).round() / 100.0,
                speaker: u.speaker.clone(),
                channel: u.channel,
                text,
                diarized: false,
                silhouette: 0.0,
                words: words.clone(),
                ctc_used: u.ctc_used,
            });
        }
        if !parts.is_empty() {
            return parts;
        }
    }

    let num_segments = bounds.len() - 1;
    if words.len() < num_segments {
        return vec![u.clone()];
    }
    let mut parts = Vec::new();
    let mut wpos = 0;
    for (text, w) in split_words_by_bounds(&words, bounds)
        .into_iter()
        .zip(bounds.windows(2))
    {
        if text.is_empty() {
            continue;
        }
        let nw = text.split_whitespace().count();
        let wconfs = u.words.get(wpos..wpos + nw).unwrap_or(&[]).to_vec();
        wpos += nw;
        parts.push(Utterance {
            start: (w[0] * 100.0).round() / 100.0,
            end: (w[1] * 100.0).round() / 100.0,
            speaker: u.speaker.clone(),
            channel: u.channel,
            text,
            diarized: false,
            silhouette: 0.0,
            words: wconfs,
            ctc_used: u.ctc_used,
        });
    }
    parts
}

/// Раскладка слов реплики по окнам между границами (доля окна от общей
/// длительности ≈ доля слов). Правая граница среза — от длины массива,
/// а не остатка: при `lo > words.len()/2` остаток даёт срез за границами массива.
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
            // Предыдущие ветки резервируют по слову на каждый оставшийся
            // фрагмент: при words.len() >= числа фрагментов остаток не пуст
            // и последний интервал не пропадает из таймлайна.
            words.len().saturating_sub(pos)
        } else {
            let remaining_segs = bounds.len() - 1 - (k + 1);
            let max_nw = words.len().saturating_sub(pos + remaining_segs).max(1);
            let raw_nw = ((words.len() as f64 * frac).round() as usize).clamp(1, max_nw);
            let mut snapped = raw_nw;
            let target = pos + raw_nw;
            let is_punct = |s: &str| s.ends_with(['.', '!', '?', ',', ';', ':']);
            if target > pos && target <= words.len() && is_punct(words[target - 1]) {
            } else if target > pos + 1 && is_punct(words[target - 2]) {
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
        cannot_link_pairs, cluster_is_impure, dedupe_cross_channel, fill_labels_by_time,
        group_segments, merge_quiet_short, merge_tiny_clusters, merge_utterances,
        move_boundary_words, profile_anchors, read_profiles_file, reconcile_channels,
        refresh_speaker_stats, short_join_threshold, speaker_change_cuts, split_at_pauses,
        split_utterance_by_bounds, split_words_by_bounds, switch_penalty, SpeakerInfo, Utterance,
    };
    use crate::transcribe::WordConf;

    fn info(name: &str, ch: i32, emb: Vec<f64>, count: usize, sec: f64) -> SpeakerInfo {
        SpeakerInfo {
            id: format!("spk_{name}"),
            name: name.to_string(),
            channel: Some(ch),
            matched: false,
            matched_name: None,
            matched_pack_id: None,
            matched_pack_name: None,
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
            words: Vec::new(),
            ctc_used: false,
        }
    }

    fn word(text: &str, start: f64, end: f64) -> WordConf {
        WordConf {
            w: text.to_string(),
            conf: 1.0,
            needs_review: false,
            start_sec: start,
            end_sec: end,
        }
    }

    #[test]
    fn boundary_words_move_to_next_utterance() {
        let mut first = utt(0.0, 2.0, "A", 1, "один два три");
        first.words = vec![
            word("один", 0.1, 0.6),
            word("два", 0.7, 1.2),
            word("три", 2.1, 2.5),
        ];
        let mut second = utt(2.0, 4.0, "B", 1, "четыре");
        second.words = vec![word("четыре", 2.6, 3.2)];
        let mut utts = vec![first, second];
        move_boundary_words(&mut utts);
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[0].text, "один два");
        assert_eq!(utts[1].text, "три четыре");
        assert_eq!(utts[1].words.len(), 2);
    }

    #[test]
    fn long_segments_cut_at_vad_minimum_in_second_half() {
        let mut probs = vec![0.9f32; 188];
        probs[31] = 0.0;
        probs[94] = 0.1;
        let out = group_segments(&[(0.0, 6.0)], &probs, 4.0, 0.5, 0);
        assert_eq!(out.len(), 2);
        assert!((out[0].1 - 94.0 * 0.032).abs() < 1e-5);
        assert_eq!(out[1].0, out[0].1);
        assert!(out.iter().all(|(s, e)| e - s <= 4.0));

        let fallback = group_segments(&[(0.0, 6.0)], &[], 4.0, 0.5, 0);
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
    fn merged_channels_centroid_uses_both_embeddings() {
        let a = mr_ml::l2_normalize(&[1.0, 0.0]);
        let b = mr_ml::l2_normalize(&[0.9, 0.1]);
        let mut infos = vec![
            info("Участник 2", 0, a.clone(), 1, 1.0),
            info("Собеседник 1", 1, b.clone(), 1, 9.0),
        ];
        let mut utts = vec![
            utt(1.0, 5.0, "Участник 2", 0, "нейтральная тестовая фраза"),
            utt(1.0, 5.0, "Собеседник 1", 1, "нейтральная тестовая фраза"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 1);
        let embedding = infos[0].embedding.clone().expect("центроид");
        let expected = mr_ml::weighted_centroid(&[a, b], &[1.0, 9.0]);
        let sim = mr_ml::cosine_similarity(&embedding, &expected);
        assert!(sim > 0.999, "слияние потеряло второй вектор: {sim}");
    }

    #[test]
    fn dedupe_cross_channel_drops_echo_even_with_different_speakers() {
        let mut utts = vec![
            utt(1.0, 5.0, "Спикер B", 1, "нейтральная тестовая фраза"),
            utt(1.1, 5.1, "Спикер A", 0, "нейтральная тестовая фраза"),
        ];
        dedupe_cross_channel(&mut utts);
        assert_eq!(utts.len(), 1);
        assert_eq!(utts[0].speaker, "Спикер B");
        assert_eq!(utts[0].channel, Some(1));
    }

    #[test]
    fn short_backchannel_over_long_phrase_survives_dedupe() {
        let mut utts = vec![
            utt(
                1.0,
                5.0,
                "Спикер B",
                1,
                "длинная нейтральная тестовая фраза",
            ),
            utt(2.0, 2.5, "Спикер A", 0, "да"),
        ];
        dedupe_cross_channel(&mut utts);
        assert_eq!(utts.len(), 2);
        assert_eq!(utts[1].speaker, "Спикер A");
    }

    #[test]
    fn full_phrase_echo_still_removed_after_short_reply_guard() {
        let mut utts = vec![
            utt(1.0, 5.0, "Спикер B", 1, "нейтральная тестовая фраза"),
            utt(1.1, 5.1, "Спикер A", 0, "нейтральная тестовая фраза"),
        ];
        dedupe_cross_channel(&mut utts);
        assert_eq!(utts.len(), 1);
        assert_eq!(utts[0].speaker, "Спикер B");
    }

    #[test]
    fn unmatched_clusters_in_one_channel_stay_separate() {
        let mut infos = vec![
            info("Участник 1", 0, vec![1.0, 0.0], 5, 10.0),
            info("Участник 2", 0, vec![1.0, 0.0], 5, 10.0),
        ];
        let mut utts = vec![
            utt(1.0, 3.0, "Участник 1", 0, "первая реплика"),
            utt(10.0, 12.0, "Участник 2", 0, "вторая реплика"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 2);
    }

    #[test]
    fn matched_profile_and_unnamed_tail_in_one_channel_merge() {
        let mut matched = info("Я", 0, vec![1.0, 0.0], 5, 10.0);
        matched.matched = true;
        matched.matched_name = Some("Я".to_string());
        let mut infos = vec![matched, info("Участник 2", 0, vec![1.0, 0.0], 1, 2.0)];
        let mut utts = vec![
            utt(1.0, 5.0, "Я", 0, "моя реплика"),
            utt(10.0, 11.0, "Участник 2", 0, "хвост"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "Я");
    }

    #[test]
    fn similar_voices_across_channels_without_overlap_stay_separate() {
        let mut infos = vec![
            info("Участник 1", 0, vec![1.0, 0.0], 1, 3.0),
            info("Собеседник 1", 1, vec![1.0, 0.0], 1, 3.0),
        ];
        let mut utts = vec![
            utt(1.0, 3.0, "Участник 1", 0, "первая реплика"),
            utt(20.0, 22.0, "Собеседник 1", 1, "вторая реплика"),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 2);
        assert_eq!(utts[0].speaker, "Участник 1");
        assert_eq!(utts[1].speaker, "Собеседник 1");
    }

    #[test]
    fn merges_short_echo_even_with_weak_embedding() {
        let mut infos = vec![
            info("Участник 5", 0, vec![1.0, 0.0], 1, 1.8),
            info("Собеседник 1", 1, vec![0.3, 0.95], 1, 3.0),
        ];
        let mut utts = vec![
            utt(10.0, 11.8, "Участник 5", 0, "Слово."),
            utt(9.8, 12.8, "Собеседник 1", 1, "Слово."),
        ];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(infos.len(), 1);
        assert_eq!(utts[0].speaker, "Собеседник 1");
        assert_eq!(utts[1].speaker, "Собеседник 1");
    }

    #[test]
    fn different_matched_profiles_are_never_merged() {
        let mut a = info("Спикер A", 0, vec![1.0, 0.0], 1, 5.0);
        a.matched = true;
        a.matched_name = Some("Спикер A".to_string());
        let mut b = info("Спикер B", 1, vec![1.0, 0.0], 1, 5.0);
        b.matched = true;
        b.matched_name = Some("Спикер B".to_string());
        let mut infos = vec![a, b];
        let mut utts = vec![
            utt(1.0, 6.0, "Спикер A", 0, "одна и та же фраза"),
            utt(1.0, 6.0, "Спикер B", 1, "одна и та же фраза"),
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
            utt(10.0, 20.0, "Я", 0, "первая часть фразы"),
            utt(19.5, 21.0, "Я", 0, "вторая часть фразы"),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].text.contains("вторая"));
        assert!(merged[0].text.contains("первая"));

        let utts = vec![
            utt(10.0, 20.0, "Я", 0, "начало длинной тестовой фразы"),
            utt(
                19.5,
                21.0,
                "Я",
                0,
                "начало длинной тестовой фразы продолжение",
            ),
        ];
        let merged = merge_utterances(utts);
        assert_eq!(merged[0].text, "начало длинной тестовой фразы продолжение");
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
        let out = merge_quiet_short(&samples, groups, 0);
        assert_eq!(out, vec![(0.0, 5.0), (6.0, 7.0), (8.0, 9.0), (10.0, 13.0)]);
    }

    #[test]
    fn merge_quiet_short_caps_merged_duration() {
        let samples = vec![0.005f32; 16 * 16000];
        let groups: Vec<(f32, f32)> = (0..8)
            .map(|i| (i as f32 * 2.0, i as f32 * 2.0 + 1.0))
            .collect();
        let out = merge_quiet_short(&samples, groups, 0);
        assert_eq!(out.len(), 2);
        assert!(out[0].1 - out[0].0 <= 12.0);
    }

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
    fn split_words_last_fragment_gets_word_when_words_remain() {
        let words = vec!["a", "b", "c", "d"];
        let texts = split_words_by_bounds(&words, &[0.0, 1.0, 4.0, 5.0]);
        assert_eq!(texts.len(), 3);
        assert!(texts.iter().all(|t| !t.is_empty()), "{texts:?}");
        assert_eq!(texts.join(" "), "a b c d");
    }

    #[test]
    fn short_join_threshold_follows_embedding_calibration() {
        assert!((short_join_threshold(0.4) - 0.36).abs() < 1e-9);
        assert!((short_join_threshold(1.0) - 0.45).abs() < 1e-9);
        assert!((short_join_threshold(1.5) - 0.48).abs() < 1e-9);
        assert!(short_join_threshold(0.6) < 0.65);
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
    fn profile_loader_recomputes_drifted_centroid_from_prototypes() {
        let dir = std::env::temp_dir().join(format!("mr-vp-drift-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("voiceprints.json");
        let mut a = vec![0.0f64; mr_ml::EMBEDDING_DIM];
        a[0] = 1.0;
        let mut b = vec![0.0f64; mr_ml::EMBEDDING_DIM];
        b[1] = 1.0;
        std::fs::write(
            &file,
            serde_json::to_string(&serde_json::json!({
                "version": 3,
                "embedding_space": mr_ml::EMBEDDING_SPACE,
                "profiles": [{
                    "name": "Свой",
                    "embedding": a.clone(),
                    "samples_count": 2,
                    "prototypes": [
                        {"id": "p1", "embedding": a, "duration_sec": 4.0, "quality": 1.0, "created_at": ""},
                        {"id": "p2", "embedding": b, "duration_sec": 4.0, "quality": 1.0, "created_at": ""},
                    ]
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        let got = read_profiles_file(&file);
        assert_eq!(got.len(), 1);
        let expected = mr_ml::weighted_centroid(&[vec![1.0, 0.0], vec![0.0, 1.0]], &[4.0, 4.0]);
        let flat: Vec<f64> = got[0].embedding[..2].to_vec();
        assert!(mr_ml::cosine_similarity(&flat, &expected) > 0.999);
        assert!((flat[0] - flat[1]).abs() < 1e-12);
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
            words: Vec::new(),
            ctc_used: false,
        };
        let out = split_at_pauses(&samples, &[utt()]);
        assert!(out.len() >= 3);
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
    fn pick_samples_prefers_well_separated_voice() {
        let mut good = utt(10.0, 14.0, "A", 0, "чистая обособленная реплика");
        good.silhouette = 0.9;
        let mut noisy = utt(1.0, 5.0, "A", 0, "чистая обособленная реплика");
        noisy.silhouette = -0.5;
        let samples = super::pick_samples(&[noisy, good], 1);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].start, 10.0);
    }

    #[test]
    fn fill_unit_labels_by_time_uses_nearest_unit() {
        let units = vec![
            super::AcoustUnit {
                utterance: 0,
                start: 0.0,
                end: 1.0,
                dur: 1.0,
                quality: 1.0,
                embedding: vec![1.0, 0.0],
            },
            super::AcoustUnit {
                utterance: 1,
                start: 5.0,
                end: 6.0,
                dur: 1.0,
                quality: 1.0,
                embedding: vec![0.0, 1.0],
            },
        ];
        let mut labels = std::collections::HashMap::from([(0usize, 7usize)]);
        super::fill_unit_labels_by_time(&units, &mut labels);
        assert_eq!(labels.get(&1), Some(&7));
    }

    #[test]
    fn cannot_link_pairs_follow_local_pyannote_speakers() {
        let frame = mr_ml::onnx::segmentation::FRAME_SAMPLES;
        let mut classes = vec![1u8; 2];
        classes.extend(vec![2u8; 2]);
        let segmentation = mr_ml::onnx::segmentation::Segmentation {
            frames: vec![],
            windows: vec![mr_ml::onnx::segmentation::SegmentationWindow {
                start_sample: 0,
                classes,
            }],
        };
        let frame_sec = frame as f64 / 16000.0;
        let utts = vec![
            utt(0.0, 2.0 * frame_sec, "A", 0, "первая"),
            utt(2.0 * frame_sec, 4.0 * frame_sec, "B", 0, "вторая"),
        ];
        let pairs = cannot_link_pairs(Some(&segmentation), &utts);
        assert_eq!(pairs, vec![(0, 1)]);
        assert!(cannot_link_pairs(None, &utts).is_empty());
    }

    #[test]
    fn split_long_finds_one_voice_change_in_four_seconds() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let wins: Vec<(f64, Vec<f64>)> = [0.0, 0.5, 1.0, 1.5, 2.0]
            .into_iter()
            .map(|t| (t, if t < 2.0 { a.clone() } else { b.clone() }))
            .collect();
        let cuts = speaker_change_cuts(0.0, 4.0, &wins);
        assert_eq!(cuts.len(), 1);
        assert!((2.0..=2.9).contains(&cuts[0]), "{cuts:?}");
    }

    #[test]
    fn switch_penalty_drops_with_pause() {
        assert!(switch_penalty(0.5) < switch_penalty(0.0));
        assert!((switch_penalty(0.0) - 0.18).abs() < 1e-12);
        assert!(switch_penalty(2.0) < 0.05);
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
    fn short_but_distinct_voice_is_not_merged_into_dominant() {
        let vecs = vec![
            vec![1.0, 0.0],
            vec![1.0, 0.0],
            vec![1.0, 0.0],
            vec![0.0, 1.0],
            vec![0.0, 1.0],
        ];
        let weights = vec![40.0, 40.0, 40.0, 0.5, 0.5];
        let mut labels = vec![0usize, 0, 0, 1, 1];
        merge_tiny_clusters(&vecs, &mut labels, &weights, 1);
        assert_eq!(labels[3], 1);
        assert_eq!(labels[4], 1);
    }

    #[test]
    fn recognized_profile_anchors_only_its_cluster() {
        let centroids =
            std::collections::HashMap::from([(0usize, vec![0.9, 0.1]), (1, vec![0.2, 0.9])]);
        let profiles = vec![mr_ml::SpeakerProfile {
            name: "A".to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
            embedding: vec![1.0, 0.0],
            samples_count: 1,
            updated_at: String::new(),
            prototypes: vec![],
        }];
        let anchors = profile_anchors(&centroids, &profiles);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[&0], vec![1.0, 0.0]);
    }

    #[test]
    fn profile_is_anchored_to_one_cluster_only() {
        let centroids =
            std::collections::HashMap::from([(0usize, vec![1.0, 0.0]), (1, vec![0.99, 0.01])]);
        let profiles = vec![mr_ml::SpeakerProfile {
            name: "A".to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
            embedding: vec![1.0, 0.0],
            samples_count: 1,
            updated_at: String::new(),
            prototypes: vec![],
        }];
        let anchors = profile_anchors(&centroids, &profiles);
        assert_eq!(anchors.len(), 1);
        assert!(anchors.contains_key(&0));
    }

    #[test]
    fn unlabelled_short_gets_nearest_time_label_without_panic() {
        let utts = vec![
            utt(0.0, 2.0, "A", 0, "a"),
            utt(10.0, 12.0, "B", 0, "b"),
            utt(9.0, 9.5, "A", 0, "short"),
        ];
        let mut labels = std::collections::HashMap::from([(0, 7), (1, 8)]);
        fill_labels_by_time(&utts, &mut labels);
        assert_eq!(labels[&2], 8);
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
        for x in samples[3 * sr..(3 * sr + (sr * 22 / 100))].iter_mut() {
            *x = 0.0;
        }
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
            words: Vec::new(),
            ctc_used: false,
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
            words: Vec::new(),
            ctc_used: false,
        };
        let out = split_at_pauses(&samples, &[u]);
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
            words: Vec::new(),
            ctc_used: false,
        };
        let out1 = split_at_pauses(&samples_closure, &[u1]);
        assert_eq!(out1.len(), 1, "90 мс пауза не должна дробить реплику");

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
            words: Vec::new(),
            ctc_used: false,
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
            words: Vec::new(),
            ctc_used: false,
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

    #[test]
    fn split_utterance_by_bounds_preserves_ctc_word_timestamps() {
        let u = Utterance {
            start: 0.0,
            end: 5.0,
            speaker: "X".to_string(),
            channel: None,
            text: "слово1 слово2 слово3 слово4".to_string(),
            diarized: false,
            silhouette: 0.0,
            words: vec![
                crate::transcribe::WordConf {
                    w: "слово1".to_string(),
                    conf: 0.99,
                    needs_review: false,
                    start_sec: 0.2,
                    end_sec: 0.6,
                },
                crate::transcribe::WordConf {
                    w: "слово2".to_string(),
                    conf: 0.98,
                    needs_review: false,
                    start_sec: 0.8,
                    end_sec: 1.2,
                },
                crate::transcribe::WordConf {
                    w: "слово3".to_string(),
                    conf: 0.97,
                    needs_review: false,
                    start_sec: 3.2,
                    end_sec: 3.6,
                },
                crate::transcribe::WordConf {
                    w: "слово4".to_string(),
                    conf: 0.96,
                    needs_review: false,
                    start_sec: 3.8,
                    end_sec: 4.2,
                },
            ],
            ctc_used: true,
        };
        let bounds = vec![0.0, 2.0, 5.0];
        let parts = split_utterance_by_bounds(&u, &bounds);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text, "слово1 слово2");
        assert_eq!(parts[0].words.len(), 2);
        assert_eq!(parts[0].words[0].w, "слово1");
        assert_eq!(parts[0].words[1].w, "слово2");
        assert_eq!(parts[1].text, "слово3 слово4");
        assert_eq!(parts[1].words.len(), 2);
        assert_eq!(parts[1].words[0].w, "слово3");
        assert_eq!(parts[1].words[1].w, "слово4");
    }

    #[test]
    fn reconcile_channels_blocks_candidate_conflict() {
        let mut infos = vec![
            SpeakerInfo {
                id: "spk_1".to_string(),
                name: "Спикер A".to_string(),
                channel: Some(0),
                matched: true,
                matched_name: Some("Спикер A".to_string()),
                matched_pack_id: None,
                matched_pack_name: None,
                similarity: 0.70,
                silhouette: 0.5,
                candidates: vec![("Спикер A".to_string(), 0.70)],
                embedding: Some(vec![1.0; mr_ml::EMBEDDING_DIM]),
                samples: vec![],
                utterances_count: 5,
                total_speech_sec: 10.0,
            },
            SpeakerInfo {
                id: "spk_2".to_string(),
                name: "Спикер B".to_string(),
                channel: Some(0),
                matched: false,
                matched_name: None,
                matched_pack_id: None,
                matched_pack_name: None,
                similarity: 0.0,
                silhouette: 0.2,
                candidates: vec![("Спикер B".to_string(), 0.55)],
                embedding: Some(vec![1.0; mr_ml::EMBEDDING_DIM]),
                samples: vec![],
                utterances_count: 3,
                total_speech_sec: 5.0,
            },
        ];
        let mut utts = vec![];
        reconcile_channels(&mut infos, &mut utts);
        assert_eq!(
            infos.len(),
            2,
            "конфликт кандидатов должен блокировать слияние"
        );
    }

    #[test]
    fn impure_cluster_needs_three_members_and_distance() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0];
        let centroid = vec![1.0, 0.0];
        assert!(!cluster_is_impure(&[&a, &b], &centroid));

        let far = vec![0.0, 1.0];
        let far2 = vec![0.0, 1.0];
        assert!(cluster_is_impure(&[&a, &far, &far2], &centroid));

        let close = vec![0.99, 0.01];
        assert!(!cluster_is_impure(&[&a, &b, &close], &centroid));
    }
}
