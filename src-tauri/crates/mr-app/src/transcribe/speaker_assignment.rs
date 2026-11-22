//! Ограниченное назначение локальных спикеров PyAnnote глобальным кластерам.

use std::collections::{HashMap, HashSet};

use mr_ml::onnx::segmentation::{Segmentation, FRAME_SAMPLES};
use mr_ml::{cosine_similarity, linear_sum_assignment_max, weighted_centroid};

use super::Utterance;

/// Ниже этого сходства собственный вектор реплики не подтверждает целевой
/// кластер: калибровка ERes2NetV2 даёт ≤ 0.22 для разных дикторов, поэтому
/// 0.30 — граница «явно чужой голос». Порог нужен только для реплик,
/// получивших собственный кластер как неизвестный голос: иначе решение окна
/// перебивает решение кластеризации.
pub(in crate::transcribe) const PROMOTED_OVERRIDE_MIN_SIM: f64 = 0.30;
/// Длинная неконфликтная реплика (≥ 1.5 с) с устойчивым сходством к своему
/// кластеру AHC не должна перетираться голосованием 10-секундных окон `PyAnnote`.
const SELF_CLUSTER_MIN_DUR: f64 = 1.5;
const SELF_CLUSTER_MIN_SIM: f64 = 0.45;
const SELF_CLUSTER_MIN_MARGIN: f64 = 0.12;
const COOCCUR_CONFLICT_MIN_OVERLAP: f64 = 0.5;
/// Порог создания нового спикера в окне совпадает с MIN_LOCAL_EMBED_SEC:
/// короткая реплика без похожего свободного голоса не плодит фантомную карточку.
const MIN_NEW_SPEAKER_SEC: f64 = 0.8;
/// При ложном разбиении монолога PyAnnote (нулевое перекрытие в окне) второй
/// локальный класс акустически явно ближе к уже занятому голосу, чем к свободному.
const MONOLOGUE_SAME_VOICE_SIM: f64 = 0.70;
const MONOLOGUE_PEER_MIN_SIM: f64 = 0.50;
const MONOLOGUE_REASSIGN_MARGIN: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SpeakerSpan {
    pub start: f64,
    pub end: f64,
    pub label: usize,
}

pub(super) struct Outcome {
    /// Смены спикера, где зона перекрытия отнесена входящему голосу.
    pub spans: Vec<SpeakerSpan>,
    /// Прогоны чистых кадров одного локального голоса, без зон перекрытия.
    pub voice_runs: Vec<SpeakerSpan>,
}

/// Венгерский алгоритм внутри каждого окна не даёт двум локальным спикерам
/// занять один глобальный кластер. При нехватке кластеров создаётся новый,
/// если это разрешено явным ограничением `max_speakers`.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    segmentation: &Segmentation,
    utterances: &[Utterance],
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
    quals: &[f64],
    label_of: &mut HashMap<usize, usize>,
    protected: &HashSet<usize>,
    max_speakers: usize,
) -> Outcome {
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
        return Outcome {
            spans: vec![],
            voice_runs: vec![],
        };
    }
    let mut next_label = globals.keys().max().map_or(0, |label| label + 1);
    let mut spans = Vec::new();
    let mut voice_runs = Vec::new();
    let mut votes: HashMap<usize, HashMap<usize, f64>> = HashMap::new();
    let mut cooccur: HashMap<(usize, usize), f64> = HashMap::new();
    let starts: Vec<f64> = utterances.iter().map(|u| u.start).collect();
    let assign_min_sim = crate::config::configured_diar_assign_min_sim();
    let new_speaker_min_sim = crate::config::configured_diar_new_speaker_min_sim();

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

        for a in 0..3 {
            for b in (a + 1)..3 {
                for &i in &local_members[a] {
                    for &j in &local_members[b] {
                        let overlap = utterances[i]
                            .end
                            .min(utterances[j].end)
                            .min(window_end as f64 / 16000.0)
                            - utterances[i]
                                .start
                                .max(utterances[j].start)
                                .max(window.start_sample as f64 / 16000.0);
                        if overlap > 0.0 {
                            // Окна PyAnnote перекрываются: сумма раздувает один
                            // и тот же стык, поэтому храним максимум.
                            let seen = cooccur.entry((i.min(j), i.max(j))).or_insert(0.0);
                            *seen = seen.max(overlap);
                        }
                    }
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
        let mut assigned_globals: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        for (local, global) in linear_sum_assignment_max(&scores) {
            if scores[local][global] >= assign_min_sim {
                let gid = global_ids[global];
                assignment.insert(locals[local].0, gid);
                assigned[local] = true;
                assigned_globals.insert(gid);
            }
        }
        let has_overlap_frames = window.classes.iter().any(|&class| (4..=6).contains(&class));
        for (row, &(local, ref embedding)) in locals.iter().enumerate() {
            if assigned[row] {
                continue;
            }
            let best_free = global_ids
                .iter()
                .filter(|&&label| !assigned_globals.contains(&label))
                .map(|&label| (label, cosine_similarity(embedding, &globals[&label])))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if !has_overlap_frames {
                let best_overall = global_ids
                    .iter()
                    .map(|&label| (label, cosine_similarity(embedding, &globals[&label])))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((overall_label, overall_sim)) = best_overall {
                    let has_cooccur = assignment
                        .iter()
                        .filter(|(_, &gid)| gid == overall_label)
                        .any(|(&other_local, _)| {
                            local_members[local].iter().any(|&i| {
                                local_members[other_local].iter().any(|&j| {
                                    i != j
                                        && cooccur
                                            .get(&(i.min(j), i.max(j)))
                                            .copied()
                                            .unwrap_or(0.0)
                                            > 0.0
                                })
                            })
                        });
                    let peers_match = locals
                        .iter()
                        .filter(|&&(other_local, _)| {
                            assignment.get(&other_local) == Some(&overall_label)
                        })
                        .all(|(_, peer_emb)| {
                            cosine_similarity(embedding, peer_emb) >= MONOLOGUE_PEER_MIN_SIM
                        });
                    let margin_ok = best_free.is_none_or(|(_, free_sim)| {
                        overall_sim - free_sim >= MONOLOGUE_REASSIGN_MARGIN
                    });
                    if assigned_globals.contains(&overall_label)
                        && !has_cooccur
                        && peers_match
                        && overall_sim >= MONOLOGUE_SAME_VOICE_SIM
                        && margin_ok
                    {
                        assignment.insert(local, overall_label);
                        continue;
                    }
                }
            }
            if let Some((label, sim)) = best_free {
                if sim >= new_speaker_min_sim {
                    assignment.insert(local, label);
                    assigned_globals.insert(label);
                    continue;
                }
            }
            let local_dur: f64 = local_members[local]
                .iter()
                .map(|&index| durs.get(index).copied().unwrap_or(0.0))
                .sum();
            if local_dur >= MIN_NEW_SPEAKER_SEC
                && (max_speakers == 0 || globals.len() < max_speakers)
            {
                globals.insert(next_label, embedding.clone());
                global_ids.push(next_label);
                assignment.insert(local, next_label);
                assigned_globals.insert(next_label);
                next_label += 1;
            }
        }
        let locals = local_frames(&window.classes);
        collect_spans(window, &locals, &assignment, &mut spans);
        collect_voice_runs(window, &assignment, &mut voice_runs);
        for (index, local, weight) in frame_weights {
            if let Some(&global) = assignment.get(&local) {
                *votes.entry(index).or_default().entry(global).or_default() += weight;
            }
        }
    }

    let (labels, conflict_moved) = resolve_label_conflicts(
        &votes,
        &cooccur,
        &starts,
        max_speakers,
        &globals,
        &mut next_label,
    );
    apply_resolved_labels(
        &labels,
        &conflict_moved,
        &cooccur,
        embs,
        durs,
        &globals,
        protected,
        label_of,
    );
    Outcome {
        spans: aggregate_spans(&spans),
        voice_runs: aggregate_spans(&voice_runs),
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_resolved_labels(
    labels: &HashMap<usize, usize>,
    conflict_moved: &HashSet<usize>,
    cooccur: &HashMap<(usize, usize), f64>,
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
    globals: &HashMap<usize, Vec<f64>>,
    protected: &HashSet<usize>,
    label_of: &mut HashMap<usize, usize>,
) {
    let mut conflict_peers: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&(i, j), &overlap) in cooccur {
        if overlap > COOCCUR_CONFLICT_MIN_OVERLAP {
            conflict_peers.entry(i).or_default().push(j);
            conflict_peers.entry(j).or_default().push(i);
        }
    }
    let mut indices: Vec<usize> = labels.keys().copied().collect();
    indices.sort_unstable();
    let mut planned = labels.clone();
    let mut deferred = Vec::new();
    for index in indices {
        let label = labels[&index];
        if protected.contains(&index) {
            let supported = embs
                .get(index)
                .and_then(Option::as_ref)
                .zip(globals.get(&label))
                .is_some_and(|(embedding, global)| {
                    cosine_similarity(embedding, global) >= PROMOTED_OVERRIDE_MIN_SIM
                });
            if !supported {
                if let Some(&old_label) = label_of.get(&index) {
                    planned.insert(index, old_label);
                }
                continue;
            }
        } else if !conflict_moved.contains(&index)
            && durs.get(index).copied().unwrap_or(0.0) >= SELF_CLUSTER_MIN_DUR
        {
            deferred.push((index, label));
            continue;
        }
        label_of.insert(index, label);
    }
    for (index, label) in deferred {
        let keep_old = label_of.get(&index).is_some_and(|&old_label| {
            let conflict = conflict_peers.get(&index).is_some_and(|peers| {
                peers.iter().any(|peer| {
                    planned
                        .get(peer)
                        .or_else(|| label_of.get(peer))
                        .is_some_and(|&peer_label| peer_label == old_label)
                })
            });
            old_label != label
                && !conflict
                && should_keep_self_cluster(
                    embs.get(index).and_then(Option::as_deref),
                    globals.get(&old_label).map(Vec::as_slice),
                    globals.get(&label).map(Vec::as_slice),
                )
        });
        if keep_old {
            if let Some(&old_label) = label_of.get(&index) {
                planned.insert(index, old_label);
            }
            continue;
        }
        label_of.insert(index, label);
    }
}

fn should_keep_self_cluster(
    embedding: Option<&[f64]>,
    old_centroid: Option<&[f64]>,
    new_centroid: Option<&[f64]>,
) -> bool {
    let (Some(embedding), Some(old_centroid), Some(new_centroid)) =
        (embedding, old_centroid, new_centroid)
    else {
        return false;
    };
    let old_sim = cosine_similarity(embedding, old_centroid);
    let new_sim = cosine_similarity(embedding, new_centroid);
    old_sim >= SELF_CLUSTER_MIN_SIM && old_sim - new_sim >= SELF_CLUSTER_MIN_MARGIN
}

/// Окна перекрываются, а локальные метки шумят. Голосуем покадрово и
/// оставляем только устойчивые отрезки: иначе короткие ошибки PyAnnote
/// рвут реплику на десятки кусков.
fn aggregate_spans(spans: &[SpeakerSpan]) -> Vec<SpeakerSpan> {
    let frame_sec = FRAME_SAMPLES as f64 / 16000.0;
    // Буферы отсчитываются от первого кадра, а не от нулевой секунды: для записи
    // в 2 ч разница — 225 тыс. кадров на каждую метку.
    let Some(origin) = spans
        .iter()
        .map(|span| (span.start.max(0.0) / frame_sec).floor() as usize)
        .min()
    else {
        return vec![];
    };
    let frames = spans
        .iter()
        .map(|s| (s.end / frame_sec).ceil() as usize)
        .max()
        .unwrap_or(0)
        .saturating_sub(origin);
    if frames == 0 {
        return vec![];
    }
    let mut labels: Vec<usize> = spans.iter().map(|s| s.label).collect();
    labels.sort_unstable();
    labels.dedup();
    let mut deltas: Vec<Vec<f32>> = vec![vec![0.0; frames + 1]; labels.len()];
    for span in spans {
        let start = ((span.start / frame_sec).floor() as usize)
            .saturating_sub(origin)
            .min(frames);
        let end = ((span.end / frame_sec).ceil() as usize)
            .saturating_sub(origin)
            .min(frames);
        if end <= start {
            continue;
        }
        let slot = labels.partition_point(|&label| label < span.label);
        deltas[slot][start] += 1.0;
        deltas[slot][end] -= 1.0;
    }
    let min_frames = ((1.0 / frame_sec).round() as usize).max(1);
    let mut counts = vec![0.0f32; labels.len()];
    let mut out = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    for frame in 0..=frames {
        let mut winner = None;
        if frame < frames {
            for (slot, delta) in deltas.iter().enumerate() {
                counts[slot] += delta[frame];
            }
            for slot in 0..labels.len() {
                if counts[slot] > 0.0 && winner.is_none_or(|best| counts[slot] > counts[best]) {
                    winner = Some(slot);
                }
            }
        }
        if matches!((winner, run), (Some(slot), Some((current, _))) if slot == current) {
            continue;
        }
        if let Some((current, start)) = run {
            if frame - start >= min_frames {
                out.push(SpeakerSpan {
                    start: (origin + start) as f64 * frame_sec,
                    end: (origin + frame) as f64 * frame_sec,
                    label: labels[current],
                });
            }
        }
        run = winner.map(|slot| (slot, frame));
    }
    out
}

/// Для overlap берём входящий голос: именно он начинает новую реплику.
/// Локальные метки по кадрам окна. Для overlap-кадров берётся ближайший чистый
/// сосед; предвычисление убирает повторный поиск того же кадра.
fn local_frames(classes: &[u8]) -> Vec<Option<usize>> {
    let mut next = vec![None; classes.len() + 1];
    for i in (0..classes.len()).rev() {
        next[i] = if (1..=3).contains(&classes[i]) {
            Some(classes[i] as usize - 1)
        } else {
            next[i + 1]
        };
    }
    let mut out = vec![None; classes.len()];
    let mut prev = None;
    for (i, &class) in classes.iter().enumerate() {
        if (1..=3).contains(&class) {
            prev = Some(class as usize - 1);
            out[i] = prev;
        } else if (4..=6).contains(&class) {
            let pair = match class {
                4 => (0, 1),
                5 => (0, 2),
                _ => (1, 2),
            };
            out[i] = next[i + 1]
                .filter(|local| *local == pair.0 || *local == pair.1)
                .or_else(|| prev.filter(|local| *local == pair.0 || *local == pair.1));
        }
    }
    out
}

fn collect_spans(
    window: &mr_ml::onnx::segmentation::SegmentationWindow,
    locals: &[Option<usize>],
    assignment: &HashMap<usize, usize>,
    out: &mut Vec<SpeakerSpan>,
) {
    let mut i = 0;
    while i < window.classes.len() {
        let Some(local) = locals[i] else {
            i += 1;
            continue;
        };
        let Some(&label) = assignment.get(&local) else {
            i += 1;
            continue;
        };
        let start = i;
        i += 1;
        while i < window.classes.len()
            && locals[i] == Some(local)
            && assignment.get(&local) == Some(&label)
        {
            i += 1;
        }
        let to_sec = |frame: usize| (window.start_sample + frame * FRAME_SAMPLES) as f64 / 16000.0;
        out.push(SpeakerSpan {
            start: to_sec(start),
            end: to_sec(i),
            label,
        });
    }
}

/// Зона перекрытия не принадлежит ни одному прогону: слово в ней атрибутируется
/// только по прогону чистой речи задетого голоса, а не по доминированию покрытия.
fn collect_voice_runs(
    window: &mr_ml::onnx::segmentation::SegmentationWindow,
    assignment: &HashMap<usize, usize>,
    out: &mut Vec<SpeakerSpan>,
) {
    let mut i = 0;
    while i < window.classes.len() {
        let class = window.classes[i];
        if !(1..=3).contains(&class) {
            i += 1;
            continue;
        }
        let Some(&label) = assignment.get(&(class as usize - 1)) else {
            i += 1;
            continue;
        };
        let start = i;
        i += 1;
        while i < window.classes.len() && window.classes[i] == class {
            i += 1;
        }
        let to_sec = |frame: usize| (window.start_sample + frame * FRAME_SAMPLES) as f64 / 16000.0;
        out.push(SpeakerSpan {
            start: to_sec(start),
            end: to_sec(i),
            label,
        });
    }
}

/// Иначе одновременно звучавшие разные голоса слипаются в «средний» профиль.
fn resolve_label_conflicts(
    votes: &HashMap<usize, HashMap<usize, f64>>,
    cooccur: &HashMap<(usize, usize), f64>,
    starts: &[f64],
    max_speakers: usize,
    globals: &HashMap<usize, Vec<f64>>,
    next_label: &mut usize,
) -> (HashMap<usize, usize>, HashSet<usize>) {
    let mut labels: HashMap<usize, usize> = HashMap::new();
    let mut conflict_moved: HashSet<usize> = HashSet::new();
    for (&index, scores) in votes {
        if let Some((&label, _)) = scores
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
        {
            labels.insert(index, label);
        }
    }
    // Лимит считаем по всем глобальным кластерам, а не только по тем, что
    // получили голоса: иначе новый ярлык создаётся поверх уже занятого лимита.
    let mut taken: std::collections::HashSet<usize> = globals.keys().copied().collect();
    taken.extend(labels.values().copied());
    let mut conflicts: Vec<((usize, usize), f64)> = cooccur
        .iter()
        .filter(|(_, &overlap)| overlap > COOCCUR_CONFLICT_MIN_OVERLAP)
        .map(|(&pair, &overlap)| (pair, overlap))
        .collect();
    conflicts.sort_by(|a, b| b.1.total_cmp(&a.1));
    for ((i, j), _) in conflicts {
        if labels.get(&i) != labels.get(&j) {
            continue;
        }
        let Some(&current) = labels.get(&i) else {
            continue;
        };
        let moved = if starts.get(i) > starts.get(j) { i } else { j };
        let second = votes.get(&moved).and_then(|scores| {
            scores
                .iter()
                .filter(|(label, _)| **label != current)
                .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(&label, _)| label)
        });
        let target = second.or_else(|| {
            (max_speakers == 0 || taken.len() < max_speakers).then(|| {
                let label = *next_label;
                *next_label += 1;
                label
            })
        });
        if let Some(label) = target {
            labels.insert(moved, label);
            conflict_moved.insert(moved);
            taken.insert(label);
        }
    }
    (labels, conflict_moved)
}

#[cfg(test)]
mod tests {
    use mr_ml::onnx::segmentation::SegmentationWindow;

    use super::*;

    fn aggregate_spans_reference(spans: &[SpeakerSpan]) -> Vec<SpeakerSpan> {
        let frame_sec = FRAME_SAMPLES as f64 / 16000.0;
        let frames = spans
            .iter()
            .map(|s| (s.end / frame_sec).ceil() as usize)
            .max()
            .unwrap_or(0);
        if frames == 0 {
            return vec![];
        }
        let mut votes: Vec<HashMap<usize, f32>> = vec![HashMap::new(); frames];
        for span in spans {
            let start = ((span.start / frame_sec).floor() as usize).min(frames);
            let end = ((span.end / frame_sec).ceil() as usize).min(frames);
            for frame in &mut votes[start..end] {
                *frame.entry(span.label).or_default() += 1.0;
            }
        }
        let best: Vec<Option<usize>> = votes
            .iter()
            .map(|frame| {
                frame
                    .iter()
                    .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
                    .map(|(&label, _)| label)
            })
            .collect();
        let min_frames = ((1.0 / frame_sec).round() as usize).max(1);
        let mut out = Vec::new();
        let mut i = 0;
        while i < best.len() {
            let Some(label) = best[i] else {
                i += 1;
                continue;
            };
            let start = i;
            i += 1;
            while i < best.len() && best[i] == Some(label) {
                i += 1;
            }
            if i - start >= min_frames {
                out.push(SpeakerSpan {
                    start: start as f64 * frame_sec,
                    end: i as f64 * frame_sec,
                    label,
                });
            }
        }
        out
    }

    #[test]
    fn aggregate_spans_matches_frame_voting_reference() {
        let span = |start: f64, end: f64, label: usize| SpeakerSpan { start, end, label };
        let cases: Vec<Vec<SpeakerSpan>> = vec![
            vec![],
            vec![span(0.0, 1.2, 1)],
            vec![span(0.0, 1.2, 5)],
            vec![span(0.0, 1.0, 1), span(0.5, 1.5, 2)],
            vec![span(0.0, 1.0, 2), span(0.5, 1.5, 1)],
            vec![span(0.0, 1.0, 1), span(1.0, 1.4, 2), span(1.4, 3.0, 1)],
            vec![span(0.0, 1.0, 1), span(2.0, 3.2, 1)],
            vec![span(0.0, 1.0, 1), span(0.8, 2.0, 2), span(1.5, 3.0, 3)],
            vec![span(0.0, 2.0, 1), span(0.0, 2.0, 2), span(1.0, 2.0, 2)],
            vec![span(0.3, 0.3, 1), span(0.0, 1.0, 2)],
        ];
        for spans in cases {
            assert_eq!(
                aggregate_spans(&spans),
                aggregate_spans_reference(&spans),
                "spans={spans:?}"
            );
        }
        let mut state = 123456789u64;
        let mut rng = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as f64 / u64::from(u32::MAX) as f64
        };
        for _ in 0..50 {
            let spans: Vec<SpeakerSpan> = (0..40)
                .map(|_| {
                    let start = (rng() * 8.0 * 50.0).floor() / 50.0;
                    let len = (rng() * 3.0 * 50.0).ceil() / 50.0;
                    span(start, start + len, 1 + (rng() * 3.0) as usize)
                })
                .collect();
            assert_eq!(
                aggregate_spans(&spans),
                aggregate_spans_reference(&spans),
                "spans={spans:?}"
            );
        }
    }

    #[test]
    fn aggregate_spans_keeps_absolute_time_of_late_spans() {
        let span = SpeakerSpan {
            start: 100.0,
            end: 103.0,
            label: 0,
        };
        let out = aggregate_spans(&[span]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!((out[0].start - 100.0).abs() < 0.05, "{out:?}");
        assert!((out[0].end - 103.0).abs() < 0.05, "{out:?}");
    }

    fn utterance(start: f64, end: f64) -> Utterance {
        Utterance {
            start,
            end,
            speaker: String::new(),
            channel: None,
            text: String::new(),
            diarized: false,
            silhouette: 0.0,
            words: Vec::new(),
            ctc_used: false,
        }
    }

    #[test]
    fn overlap_frame_goes_to_incoming_speaker() {
        let mut classes = vec![1u8; 10];
        classes.extend(vec![4u8; 5]);
        classes.extend(vec![2u8; 10]);
        let locals = local_frames(&classes);
        assert_eq!(locals[12], Some(1));
        assert_eq!(locals[2], Some(0));
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
        let outcome = apply(
            &segmentation,
            &utterances,
            &embs,
            &[5.0, 5.0],
            &[1.0, 1.0],
            &mut labels,
            &HashSet::new(),
            2,
        );
        assert_ne!(labels[&0], labels[&1]);
        assert!(outcome.spans.iter().any(|s| s.label == labels[&0]));
        assert!(outcome.spans.iter().any(|s| s.label == labels[&1]));
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
            &HashSet::new(),
            1,
        );
        assert_eq!(labels[&0], labels[&1]);
    }

    #[test]
    fn voice_runs_exclude_overlap_frames() {
        let mut classes = vec![1u8; 296];
        classes.extend(vec![4u8; 30]);
        classes.extend(vec![2u8; 293]);
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
        let outcome = apply(
            &segmentation,
            &utterances,
            &embs,
            &[5.0, 5.0],
            &[1.0, 1.0],
            &mut labels,
            &HashSet::new(),
            2,
        );
        let frame = FRAME_SAMPLES as f64 / 16000.0;
        let first = outcome
            .voice_runs
            .iter()
            .find(|run| run.label == labels[&0])
            .unwrap();
        assert!(first.end <= 296.0 * frame + 1e-6);
        let second = outcome
            .voice_runs
            .iter()
            .find(|run| run.label == labels[&1])
            .unwrap();
        assert!(second.start >= 325.0 * frame - 1e-9);
        let mid_overlap = 311.0 * frame;
        assert!(outcome
            .voice_runs
            .iter()
            .all(|run| run.end <= mid_overlap || run.start > mid_overlap));
        let incoming = outcome
            .spans
            .iter()
            .find(|span| span.label == labels[&1])
            .unwrap();
        assert!(incoming.start <= 296.0 * frame + 1e-6);
    }

    #[test]
    fn simultaneous_different_locals_do_not_share_label() {
        let votes = HashMap::from([
            (0, HashMap::from([(10, 6.0), (11, 1.0)])),
            (1, HashMap::from([(10, 6.0), (11, 5.0)])),
        ]);
        let cooccur = HashMap::from([((0, 1), 2.0)]);
        let mut next = 12;
        let (labels, _) =
            resolve_label_conflicts(&votes, &cooccur, &[0.0, 4.0], 0, &HashMap::new(), &mut next);
        assert_eq!(labels[&0], 10);
        assert_eq!(labels[&1], 11);
    }

    #[test]
    fn conflict_without_second_candidate_gets_new_label() {
        let votes = HashMap::from([
            (0, HashMap::from([(10, 6.0)])),
            (1, HashMap::from([(10, 6.0)])),
        ]);
        let cooccur = HashMap::from([((0, 1), 1.0)]);
        let mut next = 20;
        let (labels, _) =
            resolve_label_conflicts(&votes, &cooccur, &[0.0, 4.0], 0, &HashMap::new(), &mut next);
        assert_eq!(labels[&0], 10);
        assert_eq!(labels[&1], 20);
        assert_eq!(next, 21);
    }

    #[test]
    fn short_overlap_does_not_split_labels() {
        let votes = HashMap::from([
            (0, HashMap::from([(10, 6.0)])),
            (1, HashMap::from([(10, 6.0)])),
        ]);
        let cooccur = HashMap::from([((0, 1), 0.4)]);
        let mut next = 20;
        let (labels, _) =
            resolve_label_conflicts(&votes, &cooccur, &[0.0, 4.0], 0, &HashMap::new(), &mut next);
        assert_eq!(labels[&0], 10);
        assert_eq!(labels[&1], 10);
    }

    #[test]
    fn several_conflicts_do_not_exceed_speaker_limit() {
        let votes = HashMap::from([
            (0, HashMap::from([(10, 6.0)])),
            (1, HashMap::from([(10, 6.0)])),
            (2, HashMap::from([(10, 6.0)])),
        ]);
        let cooccur = HashMap::from([((0, 1), 1.0), ((0, 2), 1.0)]);
        let mut next = 11;
        let (labels, _) = resolve_label_conflicts(
            &votes,
            &cooccur,
            &[0.0, 4.0, 8.0],
            2,
            &HashMap::new(),
            &mut next,
        );
        let unique: std::collections::HashSet<usize> = labels.values().copied().collect();
        assert_eq!(unique.len(), 2);
    }

    #[test]
    fn unvoted_global_counts_toward_speaker_limit() {
        let votes = HashMap::from([
            (0, HashMap::from([(10, 6.0)])),
            (1, HashMap::from([(10, 6.0)])),
        ]);
        let cooccur = HashMap::from([((0, 1), 1.0)]);
        let globals = HashMap::from([(10, vec![1.0, 0.0]), (11, vec![0.0, 1.0])]);
        let mut next = 12;
        let (labels, _) =
            resolve_label_conflicts(&votes, &cooccur, &[0.0, 4.0], 2, &globals, &mut next);
        assert_eq!(labels[&0], 10);
        assert_eq!(labels[&1], 10);
        assert_eq!(next, 12);
    }

    #[test]
    fn promoted_voice_keeps_label_without_acoustic_support() {
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes: vec![1; 300],
            }],
        };
        let utterances = vec![
            utterance(0.0, 1.0),
            utterance(1.0, 2.0),
            utterance(2.0, 3.0),
        ];
        let embs = vec![
            Some(vec![1.0, 0.0]),
            Some(vec![1.0, 0.0]),
            Some(vec![0.0, 1.0]),
        ];
        let mut labels = HashMap::from([(0usize, 0usize), (1, 0), (2, 1)]);
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[1.0, 1.0, 1.0],
            &[1.0, 1.0, 1.0],
            &mut labels,
            &HashSet::from([2usize]),
            0,
        );
        assert_eq!(labels[&2], 1, "чужой голос нельзя вернуть соседу по окну");
    }

    #[test]
    fn promoted_voice_follows_supported_window_vote() {
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes: vec![1; 300],
            }],
        };
        let utterances = vec![
            utterance(0.0, 1.0),
            utterance(1.0, 2.0),
            utterance(2.0, 3.0),
        ];
        let embs = vec![
            Some(vec![1.0, 0.0]),
            Some(vec![1.0, 0.0]),
            Some(vec![0.8, 0.2]),
        ];
        let mut labels = HashMap::from([(0usize, 0usize), (1, 0), (2, 1)]);
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[1.0, 1.0, 1.0],
            &[1.0, 1.0, 1.0],
            &mut labels,
            &HashSet::from([2usize]),
            0,
        );
        assert_eq!(labels[&2], 0, "подтверждённый голос должен следовать окну");
    }

    #[test]
    fn window_vote_keeps_clean_utterance_with_strong_self_cluster() {
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes: vec![1; 350],
            }],
        };
        let utterances = vec![
            utterance(0.0, 2.5),
            utterance(2.5, 5.0),
            utterance(5.0, 7.0),
            utterance(12.0, 14.5),
        ];
        let embs = vec![
            Some(vec![1.0, 0.0, 0.0]),
            Some(vec![1.0, 0.0, 0.0]),
            Some(vec![0.1, 0.995, 0.0]),
            Some(vec![0.0, 1.0, 0.0]),
        ];
        let mut labels = HashMap::from([(0usize, 0usize), (1, 0), (2, 1), (3, 1)]);
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[2.5, 2.5, 2.0, 2.5],
            &[1.0; 4],
            &mut labels,
            &HashSet::new(),
            0,
        );
        assert_eq!(labels[&2], 1);
    }

    #[test]
    fn window_vote_updates_short_weak_and_conflicting_utterances() {
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes: vec![1; 550],
            }],
        };
        let utterances = vec![
            utterance(0.0, 2.5),
            utterance(2.5, 5.0),
            utterance(5.0, 6.2),
            utterance(6.2, 8.2),
            utterance(8.2, 10.2),
            utterance(15.0, 23.0),
        ];
        let embs = vec![
            Some(vec![1.0, 0.0, 0.0]),
            Some(vec![1.0, 0.0, 0.0]),
            Some(vec![0.0, 1.0, 0.0]),
            Some(vec![0.1, 0.2, 0.975]),
            Some(vec![0.73, 0.68, 0.0]),
            Some(vec![0.0, 1.0, 0.0]),
        ];
        let mut labels = HashMap::from([(0usize, 0usize), (1, 0), (2, 1), (3, 1), (4, 1), (5, 1)]);
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[2.5, 2.5, 1.2, 2.0, 2.0, 8.0],
            &[1.0; 6],
            &mut labels,
            &HashSet::new(),
            0,
        );
        assert_eq!(labels[&2], 0);
        assert_eq!(labels[&3], 0);
        assert_eq!(labels[&4], 0);

        let mut w0 = vec![1u8; 296];
        w0.extend(vec![2u8; 293]);
        let conflict_seg = Segmentation {
            frames: vec![],
            windows: vec![
                SegmentationWindow {
                    start_sample: 0,
                    classes: w0,
                },
                SegmentationWindow {
                    start_sample: 16000 * 6,
                    classes: vec![1u8; 590],
                },
            ],
        };
        let conflict_utts = vec![
            utterance(0.0, 5.0),
            utterance(4.0, 11.0),
            utterance(20.0, 25.0),
        ];
        let conflict_embs = vec![
            Some(vec![1.0, 0.0]),
            Some(vec![0.9, 0.436]),
            Some(vec![0.4, 0.916]),
        ];
        let mut conflict_labels = HashMap::from([(0usize, 0usize), (1, 0), (2, 1)]);
        apply(
            &conflict_seg,
            &conflict_utts,
            &conflict_embs,
            &[5.0, 7.0, 5.0],
            &[1.0; 3],
            &mut conflict_labels,
            &HashSet::new(),
            0,
        );
        assert_ne!(conflict_labels[&0], conflict_labels[&1]);
    }

    #[test]
    fn self_cluster_retention_does_not_rejoin_cooccurring_conflict_neighbor() {
        let mut w0 = vec![1u8; 255];
        w0.extend(vec![2u8; 335]);
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![
                SegmentationWindow {
                    start_sample: 0,
                    classes: w0,
                },
                SegmentationWindow {
                    start_sample: 16000 * 4,
                    classes: vec![1u8; 590],
                },
            ],
        };
        let utterances = vec![
            utterance(2.5, 5.0),
            utterance(4.0, 7.0),
            utterance(25.0, 33.0),
            utterance(0.0, 4.0),
            utterance(10.0, 14.0),
            utterance(35.0, 43.0),
        ];
        let embs = vec![
            Some(vec![0.2, 0.98]),
            Some(vec![0.8, 0.6]),
            Some(vec![0.0, 1.0]),
            Some(vec![1.0, 0.0]),
            Some(vec![1.0, 0.0]),
            Some(vec![1.0, 0.0]),
        ];
        let mut labels = HashMap::from([(0usize, 1usize), (1, 0), (2, 1), (3, 0), (4, 0), (5, 0)]);
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[2.5, 3.0, 8.0, 8.0, 7.0, 8.0],
            &[1.0; 6],
            &mut labels,
            &HashSet::new(),
            0,
        );
        assert_eq!(labels[&1], 1);
        assert_eq!(labels[&0], 0);
        assert_ne!(labels[&0], labels[&1]);
    }

    #[test]
    fn non_overlapping_monologue_split_stays_with_overall_voice() {
        let mut w0 = vec![1u8; 296];
        w0.extend(vec![2u8; 178]);
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes: w0,
            }],
        };
        let utterances = vec![
            utterance(0.0, 5.0),
            utterance(5.0, 8.0),
            utterance(20.0, 28.0),
        ];
        let embs = vec![
            Some(vec![1.0, 0.0]),
            Some(vec![1.0, 0.0]),
            Some(vec![0.0, 1.0]),
        ];
        let mut labels = HashMap::from([(0usize, 0usize), (1, 0), (2, 1)]);
        apply(
            &segmentation,
            &utterances,
            &embs,
            &[5.0, 3.0, 8.0],
            &[1.0; 3],
            &mut labels,
            &HashSet::new(),
            0,
        );
        assert_eq!(labels[&0], 0);
        assert_eq!(labels[&1], 0);
        assert_eq!(labels[&2], 1);
    }

    #[test]
    fn short_unassigned_local_does_not_spawn_new_global_speaker() {
        let mut w0 = vec![1u8; 296];
        w0.extend(vec![2u8; 36]);
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes: w0,
            }],
        };
        let utterances = vec![utterance(0.0, 5.0), utterance(5.0, 5.6)];
        let embs = vec![Some(vec![1.0, 0.0]), Some(vec![0.0, 1.0])];
        let mut labels = HashMap::from([(0usize, 0usize)]);
        let outcome = apply(
            &segmentation,
            &utterances,
            &embs,
            &[5.0, 0.6],
            &[1.0; 2],
            &mut labels,
            &HashSet::new(),
            0,
        );
        assert_eq!(labels[&0], 0);
        assert_eq!(labels.get(&1), None);
        assert!(outcome.voice_runs.iter().all(|span| span.label == 0));
    }

    #[test]
    fn single_speaker_non_overlapping_monologue_split_stays_with_overall_voice() {
        let mut w0 = vec![1u8; 296];
        w0.extend(vec![2u8; 178]);
        let segmentation = Segmentation {
            frames: vec![],
            windows: vec![SegmentationWindow {
                start_sample: 0,
                classes: w0,
            }],
        };
        let utterances = vec![utterance(0.0, 5.0), utterance(5.0, 8.0)];
        let embs = vec![Some(vec![1.0, 0.0]), Some(vec![0.99, 0.14])];
        let mut labels = HashMap::from([(0usize, 0usize)]);
        let outcome = apply(
            &segmentation,
            &utterances,
            &embs,
            &[5.0, 3.0],
            &[1.0; 2],
            &mut labels,
            &HashSet::new(),
            0,
        );
        assert_eq!(labels[&0], 0);
        assert_eq!(labels[&1], 0);
        assert!(outcome.voice_runs.iter().all(|span| span.label == 0));
    }
}
