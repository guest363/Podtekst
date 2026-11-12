use super::super::Segmentation;
use super::*;
use mr_ml::onnx::segmentation::FRAME_SAMPLES;
use std::collections::HashMap;

fn unit(utterance: usize, start: f64, end: f64) -> AcoustUnit {
    AcoustUnit {
        utterance,
        start,
        end,
        dur: end - start,
        quality: 1.0,
        embedding: vec![1.0, 0.0],
    }
}

#[test]
fn short_local_windows_cannot_seed_clusters_when_turn_anchors_exist() {
    let units = vec![
        unit(0, 0.0, 4.0),
        unit(0, 0.0, 1.8),
        unit(1, 4.0, 9.0),
        unit(1, 4.0, 5.8),
        unit(2, 10.0, 11.5),
    ];
    assert_eq!(core_indices(&units, &[0, 2, 4]), vec![0, 2, 4]);
}

#[test]
fn a_long_turn_does_not_exclude_a_second_voices_short_turns() {
    let mut units = vec![
        unit(0, 0.0, 5.0),
        unit(1, 5.0, 6.5),
        unit(2, 7.0, 8.5),
        unit(3, 9.0, 10.5),
    ];
    for unit in &mut units[1..] {
        unit.embedding = vec![0.0, 1.0];
    }
    assert_eq!(core_indices(&units, &[0, 1, 2, 3]), vec![0, 1, 2, 3]);
}

#[test]
fn short_turns_remain_available_when_long_support_is_absent() {
    let units = vec![unit(0, 0.0, 1.5), unit(1, 2.0, 2.6)];
    assert_eq!(core_indices(&units, &[0, 1]), vec![0]);
    assert_eq!(core_indices(&units[1..], &[0]), vec![0]);
}

#[test]
fn clean_support_is_capped_without_crossing_turn_boundaries() {
    let frame_sec = FRAME_SAMPLES as f64 / 16000.0;
    let segmentation = Segmentation {
        frames: (0..1000)
            .map(|index| if index % 3 == 0 { 4 } else { 1 })
            .collect(),
        windows: Vec::new(),
    };
    let support = SpeechSupport {
        vad_probs: None,
        segmentation: Some(&segmentation),
    };
    let (start, end) = support_bounds(2.0, 12.0, support);
    let duration = support.duration(&[(start, end)]);
    assert!(start >= 2.0 && end <= 12.0);
    assert!((duration - MAX_ANCHOR_SEC).abs() <= 1.0 / 16000.0);
    assert!(end - start > MAX_ANCHOR_SEC);
    assert_eq!(
        support_bounds(2.0, 2.0 + frame_sec, support),
        (2.0, 2.0 + frame_sec)
    );
}

#[test]
fn support_never_combines_established_neighboring_turns() {
    let support = SpeechSupport::default();
    let first = support_bounds(0.0, 4.0, support);
    let second = support_bounds(4.0, 12.0, support);
    assert_eq!(first, (0.0, 4.0));
    assert!(second.0 >= 4.0 && second.1 <= 12.0);
    assert!((support.duration(&[second]) - MAX_ANCHOR_SEC).abs() <= 1.0 / 16000.0);
}

#[test]
fn local_refinement_replaces_anchor_coverage_and_keeps_failed_local_fallback() {
    let units = vec![
        unit(0, 0.0, 6.0),
        unit(0, 0.0, 1.8),
        unit(0, 1.8, 3.6),
        unit(1, 6.0, 10.0),
    ];
    let mut labels = HashMap::from([(0, 0), (1, 0), (2, 1), (3, 1)]);
    retain_refinement_labels(&units, &[0, 3], &mut labels);
    assert_eq!(labels, HashMap::from([(1, 0), (2, 1), (3, 1)]));
}
