use std::collections::HashMap;

use mr_ml::interval_gap;

use crate::transcribe::{SpeakerInfo, Utterance};

use super::super::AcoustUnit;

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
