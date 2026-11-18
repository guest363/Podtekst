use std::collections::{BTreeMap, HashMap, HashSet};

use mr_ml::interval_gap;

use crate::transcribe::clean_speech::SpeechSupport;
use crate::transcribe::echo::{segment_profile, EchoEvidence};
use crate::transcribe::{SpeakerInfo, Utterance};

use super::super::AcoustUnit;

/// Минимальная доля чисто локальных кадров при сильном эхе: без порога
/// остаточное эхо на микрофоне порождает фиктивные кластеры «Участник N».
const MIN_PROMOTE_LOCAL_ONLY: f32 = 0.25;

pub(crate) fn rms_quality(chunk: &[f32]) -> f64 {
    if chunk.is_empty() {
        return 0.5;
    }
    let rms = (chunk.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / chunk.len() as f64).sqrt();
    (rms / 0.05).clamp(0.3, 1.5)
}

pub(crate) fn switch_penalty(gap: f64) -> f64 {
    const BASE: f64 = 0.18;
    BASE * (-gap.max(0.0) / 0.8).exp()
}

/// Единственная карточка канала, когда разметить голоса нечем: все реплики
/// получают одно имя, и статистика считается по факту.
pub(crate) fn fallback_info(
    name: &str,
    channel: Option<i32>,
    utterances: &[Utterance],
) -> SpeakerInfo {
    SpeakerInfo {
        id: "channel_x".to_string(),
        name: name.to_string(),
        channel,
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
    }
}

pub(crate) fn fill_labels_by_time(utterances: &[Utterance], label_of: &mut HashMap<usize, usize>) {
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

pub(crate) fn fill_unit_labels_by_time(units: &[AcoustUnit], label_of: &mut HashMap<usize, usize>) {
    let mut labeled: Vec<usize> = label_of.keys().copied().collect();
    if labeled.is_empty() {
        return;
    }
    labeled.sort_by(|&a, &b| units[a].start.total_cmp(&units[b].start));
    for (index, unit) in units.iter().enumerate() {
        if label_of.contains_key(&index) {
            continue;
        }
        // Только внутри своей реплики: сосед по времени из чужой реплики —
        // это другой голос, а не продолжение окна.
        let mut best: Option<(f64, usize)> = None;
        for &other in &labeled {
            if units[other].utterance != unit.utterance {
                continue;
            }
            let gap = interval_gap(unit.start, unit.end, units[other].start, units[other].end);
            if best.is_none_or(|(best_gap, _)| gap < best_gap) {
                best = Some((gap, other));
            }
        }
        let Some((_, other)) = best else {
            continue;
        };
        if let Some(&label) = label_of.get(&other) {
            label_of.insert(index, label);
            labeled.push(index);
        }
    }
}

/// Реплика, целиком не попавшая ни в один кластер, — отдельный голос, а не
/// продолжение соседа по времени. Иначе `fill_unit_labels_by_time` копирует
/// метку ближайшей реплики, и короткая вставка незнакомого собеседника
/// получает имя соседа (P10: решение принимается на слое кластеризации).
/// Порог длительности тот же, что у эмбеддинга: короче — реплика-реакция,
/// её оставляем соседу. Возвращает метки созданных кластеров.
pub(in crate::transcribe::diarize) fn promote_unmatched_utterances(
    units: &[AcoustUnit],
    label_of: &mut HashMap<usize, usize>,
    next_label: &mut usize,
    max_speakers: usize,
    support: SpeechSupport<'_>,
    echo: Option<&EchoEvidence>,
) -> Vec<usize> {
    let mut unmatched: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, unit) in units.iter().enumerate() {
        if !label_of.contains_key(&index) {
            unmatched.entry(unit.utterance).or_default().push(index);
        }
    }
    let mut groups: Vec<(Vec<usize>, f64, Vec<f64>)> = Vec::new();
    for (utterance, indices) in unmatched {
        // Частично размеченную реплику добирает fill_unit_labels_by_time.
        if units
            .iter()
            .enumerate()
            .any(|(index, unit)| unit.utterance == utterance && label_of.contains_key(&index))
        {
            continue;
        }
        let (start, end) = indices
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &index| {
                (lo.min(units[index].start), hi.max(units[index].end))
            });
        let previous_end = units
            .iter()
            .enumerate()
            .filter(|(index, unit)| label_of.contains_key(index) && unit.end <= start)
            .map(|(_, unit)| unit.end)
            .fold(f64::NEG_INFINITY, f64::max);
        // Вплотную к предыдущей реплике — это её продолжение, разрезанное
        // паузой, а не новый голос: продолжение наследует соседа по времени.
        if start - previous_end < 0.15 {
            continue;
        }
        if let Some(ev) = echo.filter(|ev| ev.is_strong()) {
            if segment_profile(ev, start, end).local_only < MIN_PROMOTE_LOCAL_ONLY {
                continue;
            }
        }
        let intervals: Vec<(f64, f64)> = indices
            .iter()
            .map(|&index| (units[index].start, units[index].end))
            .collect();
        let clean = support.duration(&intervals);
        if clean < super::super::MIN_LOCAL_EMBED_SEC {
            continue;
        }
        let vectors: Vec<Vec<f64>> = indices
            .iter()
            .map(|&index| units[index].embedding.clone())
            .collect();
        let weights: Vec<f64> = indices
            .iter()
            .map(|&index| units[index].dur.clamp(0.4, 4.0) * units[index].quality)
            .collect();
        let centroid = mr_ml::weighted_centroid(&vectors, &weights);
        groups.push((indices, clean, centroid));
    }
    groups.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0.first().cmp(&b.0.first()))
    });
    let mut clusters: Vec<(Vec<usize>, f64, Vec<f64>)> = Vec::new();
    for (indices, clean, centroid) in groups {
        let best = clusters
            .iter()
            .enumerate()
            .map(|(slot, (_, _, candidate))| (slot, mr_ml::cosine_similarity(&centroid, candidate)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((slot, _)) =
            best.filter(|(_, similarity)| *similarity >= mr_ml::SAME_SPEAKER_MERGE_SIM)
        {
            clusters[slot].0.extend(indices);
            clusters[slot].1 += clean;
            let members = &clusters[slot].0;
            let vectors: Vec<Vec<f64>> = members
                .iter()
                .map(|&index| units[index].embedding.clone())
                .collect();
            let weights: Vec<f64> = members
                .iter()
                .map(|&index| units[index].dur.clamp(0.4, 4.0) * units[index].quality)
                .collect();
            clusters[slot].2 = mr_ml::weighted_centroid(&vectors, &weights);
        } else {
            clusters.push((indices, clean, centroid));
        }
    }
    let mut used: HashSet<usize> = label_of.values().copied().collect();
    let mut promoted = Vec::new();
    for (indices, _, _) in clusters {
        if max_speakers > 0 && used.len() >= max_speakers {
            break;
        }
        let label = *next_label;
        *next_label += 1;
        used.insert(label);
        for index in indices {
            label_of.insert(index, label);
        }
        promoted.push(label);
    }
    promoted
}

/// Реплики, которые остались за неизвестными голосами, по их меткам.
/// Индексы реплик меняет `split_covered_utterances`, метки — нет.
pub(in crate::transcribe::diarize) fn promoted_utterances(
    label_of: &HashMap<usize, usize>,
    promoted: &HashSet<usize>,
) -> HashMap<usize, usize> {
    label_of
        .iter()
        .filter_map(|(&utterance, &label)| promoted.contains(&label).then_some((utterance, label)))
        .collect()
}

pub(crate) fn log_cluster_summary(
    ch: i32,
    k_req: Option<usize>,
    vecs: &[Vec<f64>],
    labels: &[usize],
    cannot_link: usize,
) {
    if !log::log_enabled!(target: "diar", log::Level::Info) || vecs.is_empty() || labels.is_empty()
    {
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

pub(crate) fn constraint_retry_needed(
    k_fixed: bool,
    full_constraints_used: bool,
    final_k: usize,
    has_pairs: bool,
) -> bool {
    !k_fixed && !full_constraints_used && final_k == 1 && has_pairs
}
