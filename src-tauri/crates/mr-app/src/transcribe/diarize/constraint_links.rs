use std::collections::{HashMap, HashSet};

use mr_ml::onnx::segmentation::FRAME_SAMPLES;

use super::super::clean_speech::SpeechSupport;
use super::{AcoustUnit, CORE_MIN_DUR};

pub(super) fn cannot_link_units(
    units: &[AcoustUnit],
    core_pos: &HashMap<usize, usize>,
    support: SpeechSupport<'_>,
) -> Vec<(usize, usize)> {
    let Some(segmentation) = support.segmentation else {
        return vec![];
    };
    const SR: f64 = 16000.0;
    let frame_sec = FRAME_SAMPLES as f64 / SR;
    let mut pairs = HashSet::new();
    let mut cross_turn_pairs = 0;
    let mut supported_memberships = 0;
    for window in &segmentation.windows {
        let window_start = window.start_sample as f64 / SR;
        let window_end =
            (window.start_sample + window.classes.len() * FRAME_SAMPLES) as f64 / SR;
        let mut members: [Vec<(usize, usize)>; 3] = Default::default();
        for (&index, &position) in core_pos {
            let unit = &units[index];
            let start = unit.start.max(window_start);
            let end = unit.end.min(window_end);
            if end <= start {
                continue;
            }
            let from =
                (((start - window_start) / frame_sec).floor() as usize).min(window.classes.len());
            let to = (((end - window_start) / frame_sec).ceil() as usize).min(window.classes.len());
            let mut intervals: [Vec<(f64, f64)>; 3] = Default::default();
            for (offset, &class) in window.classes[from..to].iter().enumerate() {
                if !(1..=3).contains(&class) {
                    continue;
                }
                let frame_start_sample = window.start_sample + (from + offset) * FRAME_SAMPLES;
                let frame_start = (frame_start_sample as f64 / SR).max(start);
                let frame_end = ((frame_start_sample + FRAME_SAMPLES) as f64 / SR).min(end);
                if frame_end <= frame_start {
                    continue;
                }
                let spans = &mut intervals[usize::from(class) - 1];
                if let Some(last) = spans.last_mut().filter(|last| last.1 == frame_start) {
                    last.1 = frame_end;
                } else {
                    spans.push((frame_start, frame_end));
                }
            }
            let overlap = intervals.map(|intervals| support.duration(&intervals));
            let total: f64 = overlap.iter().sum();
            let Some((local, &best)) = overlap.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))
            else {
                continue;
            };
            if best >= CORE_MIN_DUR && best / total >= 0.6 {
                members[local].push((position, unit.utterance));
                supported_memberships += 1;
            }
        }
        for a in 0..3 {
            for b in (a + 1)..3 {
                for &(i, utt_a) in &members[a] {
                    for &(j, utt_b) in &members[b] {
                        if pairs.insert((i.min(j), i.max(j))) && utt_a != utt_b {
                            cross_turn_pairs += 1;
                        }
                    }
                }
            }
        }
    }
    let mut pairs: Vec<_> = pairs.into_iter().collect();
    pairs.sort_unstable();
    log::debug!(
        target: "diar",
        windows = segmentation.windows.len(),
        core_units = core_pos.len(),
        supported_memberships = supported_memberships,
        pairs = pairs.len(),
        cross_turn_pairs = cross_turn_pairs;
        "ev=diar.cannot_link"
    );
    pairs
}

#[cfg(test)]
mod tests;
