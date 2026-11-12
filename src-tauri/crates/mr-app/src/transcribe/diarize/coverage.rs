use super::super::clean_speech::SpeechSupport;
use super::{AcoustUnit, Utterance};
use std::collections::HashMap;

const MIN_CLUSTER_SHARE: f64 = 0.05;
const MIN_CLUSTER_SEC: f64 = 1.5;
const MIN_TURN_SHARE: f64 = 0.25;
const MIN_TURN_SEC: f64 = 1.0;

fn speech_duration(intervals: &[(f64, f64)], support: SpeechSupport<'_>) -> f64 {
    support.duration(intervals)
}

#[derive(Debug)]
pub(super) struct CoreClustering {
    pub core_units: Vec<usize>,
    pub labels: Vec<usize>,
}

pub(super) fn reject_small_clusters(
    units: &[AcoustUnit],
    core: &[usize],
    mut labels: Vec<usize>,
    weights: &[f64],
    constraints: &[(usize, usize)],
    support: SpeechSupport<'_>,
    speaker_bounds: std::ops::RangeInclusive<usize>,
) -> CoreClustering {
    let mut core = core.to_vec();
    let mut weights = weights.to_vec();
    let mut constraints = constraints.to_vec();
    loop {
        let unit_labels: HashMap<_, _> = core.iter().copied().zip(labels.iter().copied()).collect();
        let mut utterance_ids: Vec<_> = core.iter().map(|&i| units[i].utterance).collect();
        utterance_ids.sort_unstable();
        utterance_ids.dedup();
        let mut intervals: HashMap<usize, Vec<(f64, f64)>> = HashMap::new();
        for utterance in utterance_ids {
            for (start, end, label) in timeline(units, &unit_labels, utterance) {
                intervals.entry(label).or_default().push((start, end));
            }
        }
        for &label in &labels {
            intervals.entry(label).or_default();
        }
        let k = intervals.len();
        if k <= 1 {
            return CoreClustering {
                core_units: core,
                labels,
            };
        }
        let all: Vec<_> = intervals.values().flatten().copied().collect();
        let total = speech_duration(&all, support);
        let protected: std::collections::HashSet<_> = constraints
            .iter()
            .flat_map(|&(a, b)| [labels[a], labels[b]])
            .collect();
        let mut rejected: std::collections::HashSet<_> = intervals
            .iter()
            .filter_map(|(&label, spans)| {
                let sec = speech_duration(spans, support);
                (!protected.contains(&label)
                    && (sec < MIN_CLUSTER_SEC || sec < total * MIN_CLUSTER_SHARE))
                    .then_some(label)
            })
            .collect();
        if rejected.len() == k {
            if let Some((&largest, _)) =
                intervals
                    .iter()
                    .max_by(|(label_a, spans_a), (label_b, spans_b)| {
                        speech_duration(spans_a, support)
                            .total_cmp(&speech_duration(spans_b, support))
                            .then_with(|| label_b.cmp(label_a))
                    })
            {
                rejected.remove(&largest);
            }
        }
        if !rejected.is_empty() && k - rejected.len() < *speaker_bounds.start() {
            log::debug!(target: "diar", minimum = *speaker_bounds.start(), retained = k - rejected.len(); "ev=diar.coverage_minimum");
        }
        if rejected.is_empty() || k - rejected.len() < *speaker_bounds.start() {
            return CoreClustering {
                core_units: core,
                labels,
            };
        }
        let mut rejected_labels: Vec<_> = rejected.iter().copied().collect();
        rejected_labels.sort_unstable();
        log::debug!(
            target: "diar",
            before = k,
            retained = k - rejected.len(),
            rejected = format!("{rejected_labels:?}").as_str(),
            speech_sec = total;
            "ev=diar.coverage_reject"
        );
        let retained: Vec<_> = labels
            .iter()
            .enumerate()
            .filter_map(|(position, label)| (!rejected.contains(label)).then_some(position))
            .collect();
        let positions: HashMap<_, _> = retained
            .iter()
            .enumerate()
            .map(|(new, &old)| (old, new))
            .collect();
        constraints = constraints
            .iter()
            .map(|&(a, b)| (positions[&a], positions[&b]))
            .collect();
        core = retained.iter().map(|&position| core[position]).collect();
        weights = retained.iter().map(|&position| weights[position]).collect();
        let vecs: Vec<_> = core
            .iter()
            .map(|&index| units[index].embedding.clone())
            .collect();
        labels = mr_ml::cluster::agglomerative_gap(
            &vecs,
            &weights,
            &constraints,
            0.52,
            *speaker_bounds.start(),
            *speaker_bounds.end(),
        );
    }
}

fn timeline(
    units: &[AcoustUnit],
    labels: &HashMap<usize, usize>,
    utterance: usize,
) -> Vec<(f64, f64, usize)> {
    let members: Vec<_> = units
        .iter()
        .enumerate()
        .filter(|(i, u)| u.utterance == utterance && labels.contains_key(i))
        .collect();
    let mut bounds: Vec<_> = members.iter().flat_map(|(_, u)| [u.start, u.end]).collect();
    for (pos, (_, a)) in members.iter().enumerate() {
        for (_, b) in &members[pos + 1..] {
            if a.start < b.end && b.start < a.end {
                let cut = (a.start + a.end + b.start + b.end) * 0.25;
                if cut > a.start.max(b.start) && cut < a.end.min(b.end) {
                    bounds.push(cut);
                }
            }
        }
    }
    bounds.sort_by(f64::total_cmp);
    bounds.dedup();
    let mut spans: Vec<(f64, f64, usize)> = Vec::new();
    for pair in bounds.windows(2) {
        let mid = (pair[0] + pair[1]) * 0.5;
        let Some((index, _)) = members
            .iter()
            .filter(|(_, u)| u.start <= mid && u.end >= mid)
            .min_by(|(ia, a), (ib, b)| {
                ((a.start + a.end) * 0.5 - mid)
                    .abs()
                    .total_cmp(&((b.start + b.end) * 0.5 - mid).abs())
                    .then_with(|| ia.cmp(ib))
            })
        else {
            continue;
        };
        let label = labels[index];
        if let Some(last) = spans.last_mut() {
            if last.2 == label && last.1 == pair[0] {
                last.1 = pair[1];
                continue;
            }
        }
        spans.push((pair[0], pair[1], label));
    }
    spans
}

pub(super) fn label_durations(
    units: &[AcoustUnit],
    labels: &HashMap<usize, usize>,
    support: SpeechSupport<'_>,
) -> HashMap<(usize, usize), f64> {
    let mut intervals: HashMap<(usize, usize), Vec<(f64, f64)>> = HashMap::new();
    for (index, unit) in units.iter().enumerate() {
        if let Some(&label) = labels.get(&index) {
            intervals
                .entry((unit.utterance, label))
                .or_default()
                .push((unit.start, unit.end));
        }
    }
    intervals
        .into_iter()
        .map(|(key, spans)| (key, speech_duration(&spans, support)))
        .collect()
}

pub(super) fn split_covered_utterances(
    utterances: &mut Vec<Utterance>,
    units: &mut Vec<AcoustUnit>,
    labels: &mut HashMap<usize, usize>,
    support: SpeechSupport<'_>,
) -> HashMap<usize, usize> {
    let mut output = Vec::new();
    let mut output_units = Vec::new();
    let mut output_labels = HashMap::new();
    let mut protected = HashMap::new();
    for (i, u) in utterances.iter().enumerate() {
        let spans = timeline(units, labels, i);
        let mut by_label: HashMap<usize, Vec<(f64, f64)>> = HashMap::new();
        for &(s, e, l) in &spans {
            by_label
                .entry(l)
                .or_default()
                .push((s.max(u.start), e.min(u.end)));
        }
        let significant: std::collections::HashSet<_> = by_label
            .iter()
            .filter_map(|(&label, intervals)| {
                let sec = speech_duration(intervals, support);
                (sec >= MIN_TURN_SEC && sec >= (u.end - u.start) * MIN_TURN_SHARE).then_some(label)
            })
            .collect();
        let mut bounds = vec![u.start];
        if significant.len() >= 2 {
            let runs: Vec<_> = spans
                .iter()
                .filter(|(_, _, label)| significant.contains(label))
                .collect();
            for pair in runs.windows(2) {
                if pair[0].2 != pair[1].2 {
                    let cut = (pair[0].1 + pair[1].0) * 0.5;
                    if cut > bounds.last().copied().unwrap_or(u.start) && cut < u.end {
                        bounds.push(cut);
                    }
                }
            }
        }
        bounds.push(u.end);
        let parts = if bounds.len() > 2 {
            super::split_utterance_by_bounds(u, &bounds)
        } else {
            vec![u.clone()]
        };
        for part in parts {
            let new_index = output.len();
            if bounds.len() > 2 {
                let mut coverage: HashMap<usize, Vec<(f64, f64)>> = HashMap::new();
                for &(start, end, label) in &spans {
                    if significant.contains(&label) {
                        coverage
                            .entry(label)
                            .or_default()
                            .push((start.max(part.start), end.min(part.end)));
                    }
                }
                if let Some((label, sec)) = coverage
                    .into_iter()
                    .map(|(label, intervals)| (label, speech_duration(&intervals, support)))
                    .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                {
                    if sec > 0.0 {
                        protected.insert(new_index, label);
                    }
                }
            }

            for (index, unit) in units
                .iter()
                .enumerate()
                .filter(|(_, unit)| unit.utterance == i)
            {
                let Some(&label) = labels.get(&index) else {
                    continue;
                };
                for &(span_start, span_end, span_label) in &spans {
                    if span_label != label {
                        continue;
                    }
                    let start = unit.start.max(part.start).max(span_start);
                    let end = unit.end.min(part.end).min(span_end);
                    if end <= start {
                        continue;
                    }
                    output_labels.insert(output_units.len(), label);
                    output_units.push(AcoustUnit {
                        utterance: new_index,
                        start,
                        end,
                        dur: speech_duration(&[(start, end)], support),
                        quality: unit.quality,
                        embedding: unit.embedding.clone(),
                    });
                }
            }
            output.push(part);
        }
    }
    *utterances = output;
    *units = output_units;
    *labels = output_labels;
    protected
}

pub(super) fn restore_protected(
    labels: &mut HashMap<usize, usize>,
    protected: &HashMap<usize, usize>,
) {
    labels.extend(protected.iter().map(|(&index, &label)| (index, label)));
}

#[cfg(test)]
mod tests;
