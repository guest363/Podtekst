use mr_ml::onnx::segmentation::{Segmentation, FRAME_SAMPLES};

pub(super) fn duration(intervals: &[(f64, f64)], segmentation: Option<&Segmentation>) -> f64 {
    let mut intervals: Vec<(usize, usize)> = intervals
        .iter()
        .filter(|(start, end)| start.is_finite() && end.is_finite() && end > start)
        .map(|(start, end)| {
            (
                (start.max(0.0) * 16000.0) as usize,
                (end.max(0.0) * 16000.0) as usize,
            )
        })
        .collect();
    intervals.sort_unstable();
    let mut covered_end = 0;
    let mut count = 0;
    for (start, end) in intervals {
        let start = start.max(covered_end);
        if end <= start {
            continue;
        }
        count += end - start;
        if let Some(segmentation) = segmentation {
            for frame in start / FRAME_SAMPLES..end.div_ceil(FRAME_SAMPLES) {
                if segmentation
                    .frames
                    .get(frame)
                    .is_some_and(|class| *class >= 4)
                {
                    count -=
                        end.min((frame + 1) * FRAME_SAMPLES) - start.max(frame * FRAME_SAMPLES);
                }
            }
        }
        covered_end = end;
    }
    count as f64 / 16000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_windows_count_each_sample_once() {
        assert_eq!(duration(&[(0.9, 2.7), (0.0, 1.8)], None), 2.7);
        assert_eq!(duration(&[(1.0, 1.8)], None), 0.8);
    }

    #[test]
    fn partial_overlap_frames_are_excluded_once() {
        let segmentation = Segmentation {
            frames: vec![1, 4, 1],
            windows: vec![],
        };
        let intervals = [(0.0, 0.04), (0.01, 0.05)];
        let expected = (800 - FRAME_SAMPLES) as f64 / 16000.0;
        assert_eq!(duration(&intervals, Some(&segmentation)), expected);
    }
}
