use mr_ml::onnx::segmentation::{Segmentation, FRAME_SAMPLES};
use mr_ml::onnx::vad::VAD_WINDOW;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SpeechSupport<'a> {
    pub vad_probs: Option<&'a [f32]>,
    pub segmentation: Option<&'a Segmentation>,
}

impl<'a> SpeechSupport<'a> {
    fn ranges(self, start: usize, end: usize) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
        let mut cursor = start;
        std::iter::from_fn(move || {
            while cursor < end {
                let start = cursor;
                let vad_frame = start / VAD_WINDOW;
                let segmentation_frame = start / FRAME_SAMPLES;
                cursor = end;
                if self.vad_probs.is_some() {
                    cursor = cursor.min((vad_frame + 1) * VAD_WINDOW);
                }
                if self.segmentation.is_some() {
                    cursor = cursor.min((segmentation_frame + 1) * FRAME_SAMPLES);
                }
                let speech = self.vad_probs.is_none_or(|probs| {
                    probs
                        .get(vad_frame)
                        .is_some_and(|&prob| prob >= super::diag::VAD_THRESHOLD)
                });
                let overlap = self.segmentation.is_some_and(|segmentation| {
                    segmentation
                        .frames
                        .get(segmentation_frame)
                        .is_some_and(|&class| class >= super::OVERLAP_CLASS)
                });
                if speech && !overlap {
                    return Some(start..cursor);
                }
            }
            None
        })
    }

    pub fn duration(self, intervals: &[(f64, f64)]) -> f64 {
        if self.vad_probs.is_none() {
            duration(intervals, self.segmentation)
        } else {
            self.masked_duration(intervals)
        }
    }

    fn masked_duration(self, intervals: &[(f64, f64)]) -> f64 {
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
            count += self
                .ranges(start, end)
                .map(|range| range.len())
                .sum::<usize>();
            covered_end = end;
        }
        count as f64 / 16000.0
    }

    pub fn retain_in_place(self, samples: &mut [f32], offset: usize) -> usize {
        let mut write = 0;
        for range in self.ranges(offset, offset + samples.len()) {
            let len = range.len();
            samples.copy_within(range.start - offset..range.end - offset, write);
            write += len;
        }
        write
    }
}

pub(super) fn duration(intervals: &[(f64, f64)], segmentation: Option<&Segmentation>) -> f64 {
    SpeechSupport {
        vad_probs: None,
        segmentation,
    }
    .masked_duration(intervals)
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

#[cfg(test)]
mod speech_support_tests {
    use super::*;
    use mr_ml::onnx::vad::VAD_WINDOW;

    #[test]
    fn vad_and_overlap_use_identical_sample_support() {
        let vad = [0.0, 1.0, 1.0, 0.0];
        let segmentation = Segmentation {
            frames: vec![0, 0, 0, 4, 0, 0, 0, 0],
            windows: vec![],
        };
        let support = SpeechSupport {
            vad_probs: Some(&vad),
            segmentation: Some(&segmentation),
        };
        let mut samples: Vec<f32> = (0..VAD_WINDOW * vad.len()).map(|i| i as f32).collect();
        let retained = support.retain_in_place(&mut samples, 0);
        let expected: Vec<f32> = (512..810).chain(1080..1536).map(|i| i as f32).collect();
        assert_eq!(retained, 754);
        assert!(samples[..retained].iter().eq(expected.iter()));
        let end = (VAD_WINDOW * vad.len()) as f64 / 16000.0;
        assert_eq!(
            support.duration(&[(0.0, end), (0.01, end)]),
            retained as f64 / 16000.0
        );
    }
    #[test]
    fn offset_and_unknown_segmentation_tail_keep_positive_vad_samples() {
        let vad = [0.0, super::super::diag::VAD_THRESHOLD];
        let segmentation = Segmentation {
            frames: vec![0],
            windows: vec![],
        };
        let support = SpeechSupport {
            vad_probs: Some(&vad),
            segmentation: Some(&segmentation),
        };
        let offset = VAD_WINDOW - 10;
        let mut samples = vec![1.0; VAD_WINDOW + 30];
        let retained = support.retain_in_place(&mut samples, offset);
        assert_eq!(retained, VAD_WINDOW);
        let intervals = [(
            offset as f64 / 16000.0,
            (offset + samples.len()) as f64 / 16000.0,
        )];
        assert_eq!(support.duration(&intervals), retained as f64 / 16000.0);
    }

    #[test]
    fn absent_vad_preserves_legacy_overlap_only_compaction() {
        let segmentation = Segmentation {
            frames: vec![0, 4, 1],
            windows: vec![],
        };
        let support = SpeechSupport {
            vad_probs: None,
            segmentation: Some(&segmentation),
        };
        let offset = 100;
        let mut actual: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let mut expected = actual.clone();
        let old_len = segmentation.exclude_overlap(&mut expected, offset);
        let new_len = support.retain_in_place(&mut actual, offset);
        assert_eq!(&actual[..new_len], &expected[..old_len]);
        assert_eq!(
            support.duration(&[(0.0, 0.1)]),
            (1600 - FRAME_SAMPLES) as f64 / 16000.0
        );
    }
}
