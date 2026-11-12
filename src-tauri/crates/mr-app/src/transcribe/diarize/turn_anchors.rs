use super::super::clean_speech::SpeechSupport;
use super::AcoustUnit;

const MAX_ANCHOR_SEC: f64 = 6.0;

fn prefix_end(start: f64, end: f64, target: f64, support: SpeechSupport<'_>) -> f64 {
    let mut lo = start;
    let mut hi = end;
    for _ in 0..32 {
        let mid = (lo + hi) * 0.5;
        if support.duration(&[(start, mid)]) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

pub(super) fn support_bounds(start: f64, end: f64, support: SpeechSupport<'_>) -> (f64, f64) {
    let duration = support.duration(&[(start, end)]);
    if duration <= MAX_ANCHOR_SEC {
        return (start, end);
    }
    let start = prefix_end(start, end, (duration - MAX_ANCHOR_SEC) * 0.5, support);
    let end = prefix_end(start, end, MAX_ANCHOR_SEC, support);
    (start, end)
}

pub(super) fn core_indices(units: &[AcoustUnit], anchors: &[usize]) -> Vec<usize> {
    let core: Vec<_> = anchors
        .iter()
        .copied()
        .filter(|&index| units[index].dur >= super::CORE_MIN_DUR)
        .collect();
    if core.is_empty() {
        anchors.to_vec()
    } else {
        core
    }
}

pub(super) fn retain_refinement_labels(
    units: &[AcoustUnit],
    anchors: &[usize],
    labels: &mut std::collections::HashMap<usize, usize>,
) {
    let anchor_set: std::collections::HashSet<_> = anchors.iter().copied().collect();
    for &index in anchors {
        let anchor = &units[index];
        let refined = units.iter().enumerate().any(|(other, unit)| {
            !anchor_set.contains(&other)
                && labels.contains_key(&other)
                && unit.utterance == anchor.utterance
                && unit.start < anchor.end
                && unit.end > anchor.start
        });
        if refined {
            labels.remove(&index);
        }
    }
}

#[cfg(test)]
mod tests;
