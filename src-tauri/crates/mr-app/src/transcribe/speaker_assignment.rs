//! Ограниченное назначение локальных спикеров PyAnnote глобальным кластерам.

use std::collections::HashMap;

use mr_ml::onnx::segmentation::{Segmentation, FRAME_SAMPLES};
use mr_ml::{cosine_similarity, linear_sum_assignment_max, weighted_centroid};

use super::Utterance;

/// Венгерский алгоритм внутри каждого окна не даёт двум локальным спикерам
/// занять один глобальный кластер. При нехватке кластеров создаётся новый,
/// если это разрешено явным ограничением `max_speakers`.
pub(super) fn apply(
    segmentation: &Segmentation,
    utterances: &[Utterance],
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
    quals: &[f64],
    label_of: &mut HashMap<usize, usize>,
    max_speakers: usize,
) {
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&index, &label) in label_of.iter() {
        if embs.get(index).is_some_and(Option::is_some) {
            members.entry(label).or_default().push(index);
        }
    }
    let mut globals: HashMap<usize, Vec<f64>> = members
        .into_iter()
        .map(|(label, indices)| {
            let vectors: Vec<Vec<f64>> = indices
                .iter()
                .filter_map(|&index| embs[index].clone())
                .collect();
            let weights: Vec<f64> = indices
                .iter()
                .map(|&index| durs[index].min(8.0) * quals[index])
                .collect();
            (label, weighted_centroid(&vectors, &weights))
        })
        .collect();
    if globals.is_empty() {
        return;
    }
    let mut next_label = globals.keys().max().map_or(0, |label| label + 1);
    let mut votes: HashMap<usize, HashMap<usize, f64>> = HashMap::new();

    for window in &segmentation.windows {
        let window_end = window.start_sample + window.classes.len() * FRAME_SAMPLES;
        let mut local_members: [Vec<usize>; 3] = Default::default();
        let mut frame_weights = vec![];
        for (index, utterance) in utterances.iter().enumerate() {
            let utterance_start = (utterance.start.max(0.0) * 16000.0) as usize;
            let utterance_end = (utterance.end.max(0.0) * 16000.0) as usize;
            if utterance_start >= window_end || utterance_end <= window.start_sample {
                continue;
            }
            let mut scores = [0.0f64; 3];
            for (frame, &class) in window.classes.iter().enumerate() {
                if !(1..=3).contains(&class) {
                    continue;
                }
                let frame_start = window.start_sample + frame * FRAME_SAMPLES;
                let frame_end = frame_start + FRAME_SAMPLES;
                let overlap = frame_end
                    .min(utterance_end)
                    .saturating_sub(frame_start.max(utterance_start));
                scores[class as usize - 1] += overlap as f64;
            }
            for (local, &weight) in scores.iter().enumerate() {
                if weight > 0.0 {
                    frame_weights.push((index, local, weight));
                }
            }
            if embs[index].is_some() {
                if let Some((local, _)) = scores
                    .iter()
                    .enumerate()
                    .filter(|(_, weight)| **weight > 0.0)
                    .max_by(|a, b| a.1.total_cmp(b.1))
                {
                    local_members[local].push(index);
                }
            }
        }

        let locals: Vec<(usize, Vec<f64>)> = local_members
            .iter()
            .enumerate()
            .filter(|(_, indices)| !indices.is_empty())
            .map(|(local, indices)| {
                let vectors: Vec<Vec<f64>> = indices
                    .iter()
                    .filter_map(|&index| embs[index].clone())
                    .collect();
                let weights: Vec<f64> = indices
                    .iter()
                    .map(|&index| durs[index].min(8.0) * quals[index])
                    .collect();
                (local, weighted_centroid(&vectors, &weights))
            })
            .collect();
        if locals.is_empty() {
            continue;
        }
        let mut global_ids: Vec<usize> = globals.keys().copied().collect();
        global_ids.sort_unstable();
        let scores: Vec<Vec<f64>> = locals
            .iter()
            .map(|(_, embedding)| {
                global_ids
                    .iter()
                    .map(|label| cosine_similarity(embedding, &globals[label]))
                    .collect()
            })
            .collect();
        let mut assignment = HashMap::new();
        let mut assigned = vec![false; locals.len()];
        for (local, global) in linear_sum_assignment_max(&scores) {
            assignment.insert(locals[local].0, global_ids[global]);
            assigned[local] = true;
        }
        for (row, (_, embedding)) in locals.iter().enumerate() {
            if !assigned[row] && (max_speakers == 0 || globals.len() < max_speakers) {
                let local = locals[row].0;
                globals.insert(next_label, embedding.clone());
                assignment.insert(local, next_label);
                next_label += 1;
            }
        }
        for (index, local, weight) in frame_weights {
            if let Some(&global) = assignment.get(&local) {
                *votes.entry(index).or_default().entry(global).or_default() += weight;
            }
        }
    }

    for (index, scores) in votes {
        if let Some((label, _)) = scores
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        {
            label_of.insert(index, label);
        }
    }
}

#[cfg(test)]
mod tests {
    use mr_ml::onnx::segmentation::SegmentationWindow;

    use super::*;

    fn utterance(start: f64, end: f64) -> Utterance {
        Utterance {
            start,
            end,
            speaker: String::new(),
            channel: None,
            text: String::new(),
            diarized: false,
            silhouette: 0.0,
        }
    }

    #[test]
    fn simultaneous_local_speakers_get_distinct_global_clusters() {
        let mut classes = vec![1; 296];
        classes.extend(vec![2; 293]);
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes,
            }],
        };
        let utterances = vec![utterance(0.0, 5.0), utterance(5.0, 10.0)];
        let embs = vec![Some(vec![1.0, 0.0]), Some(vec![0.0, 1.0])];
        let mut labels = [(0, 0), (1, 0)].into_iter().collect();
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[5.0, 5.0],
            &[1.0, 1.0],
            &mut labels,
            2,
        );
        assert_ne!(labels[&0], labels[&1]);
    }

    #[test]
    fn explicit_speaker_limit_is_preserved() {
        let mut classes = vec![1; 296];
        classes.extend(vec![2; 293]);
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes,
            }],
        };
        let utterances = vec![utterance(0.0, 5.0), utterance(5.0, 10.0)];
        let embs = vec![Some(vec![1.0, 0.0]), Some(vec![0.0, 1.0])];
        let mut labels = [(0, 0), (1, 0)].into_iter().collect();
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[5.0, 5.0],
            &[1.0, 1.0],
            &mut labels,
            1,
        );
        assert_eq!(labels[&0], labels[&1]);
    }
}
