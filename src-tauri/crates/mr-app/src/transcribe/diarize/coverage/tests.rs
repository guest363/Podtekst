use super::*;
use mr_ml::onnx::segmentation::Segmentation;

fn reject_small_clusters(
    units: &[AcoustUnit],
    core: &[usize],
    labels: Vec<usize>,
    weights: &[f64],
    constraints: &[(usize, usize)],
    segmentation: Option<&Segmentation>,
) -> CoreClustering {
    super::reject_small_clusters(
        units,
        core,
        labels,
        weights,
        constraints,
        SpeechSupport {
            vad_probs: None,
            segmentation,
        },
        1..=8,
    )
}

fn unit(utterance: usize, start: f64, end: f64, label: usize) -> AcoustUnit {
    AcoustUnit {
        utterance,
        start,
        end,
        dur: end - start,
        quality: 1.0,
        embedding: if label == 0 {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        },
    }
}

#[test]
fn rejects_far_outlier_without_merging_supported_speakers() {
    let mut units = vec![
        unit(0, 0.0, 10.0, 0),
        unit(1, 10.0, 20.0, 1),
        unit(2, 20.0, 21.0, 0),
    ];
    units[2].embedding = vec![-std::f64::consts::FRAC_1_SQRT_2; 2];
    let labels = reject_small_clusters(
        &units,
        &[0, 1, 2],
        vec![0, 1, 2],
        &[4.0, 4.0, 1.0],
        &[],
        None,
    );
    assert_ne!(
        labels.labels[0], labels.labels[1],
        "supported speakers must stay distinct"
    );
    assert_eq!(
        labels
            .labels
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2,
        "one unsupported outlier must not increase the speaker count"
    );
}

#[test]
fn rejects_cluster_under_five_percent_even_when_distinct() {
    let units = vec![unit(0, 0.0, 60.0, 0), unit(1, 60.0, 62.9, 1)];
    let labels = reject_small_clusters(&units, &[0, 1], vec![0, 1], &[4.0, 2.9], &[], None);
    assert_eq!(
        labels.core_units,
        vec![0],
        "2.9s minority must leave core below 5%"
    );
}

#[test]
fn rejects_cluster_under_absolute_minimum_even_above_five_percent() {
    let units = vec![unit(0, 0.0, 8.0, 0), unit(1, 8.0, 9.2, 1)];
    let labels = reject_small_clusters(&units, &[0, 1], vec![0, 1], &[4.0, 1.2], &[], None);
    assert_eq!(
        labels.core_units,
        vec![0],
        "1.2s minority cannot validate k=2"
    );
}

#[test]
fn splits_quarter_coverage_preserving_word_timestamps_and_text() {
    let mut u = super::super::tests::utt(0.0, 8.0, "", 0, "one two three four");
    u.words = vec![
        super::super::tests::word("one", 0.0, 1.0),
        super::super::tests::word("two", 2.0, 3.0),
        super::super::tests::word("three", 4.0, 5.0),
        super::super::tests::word("four", 6.0, 7.0),
    ];
    u.ctc_used = true;
    let original_words = u.words.clone();
    let mut utterances = vec![u];
    let mut units = vec![unit(0, 0.0, 6.0, 0), unit(0, 6.0, 8.0, 1)];
    let mut labels = HashMap::from([(0, 0), (1, 1)]);
    split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut labels,
        SpeechSupport::default(),
    );
    assert_eq!(
        utterances.len(),
        2,
        "25% minority must survive majority assignment"
    );
    assert_eq!((utterances[0].end, utterances[1].start), (6.0, 6.0));
    assert_eq!(
        utterances
            .iter()
            .map(|u| u.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        "one two three four"
    );
    let words: Vec<_> = utterances.iter().flat_map(|u| u.words.iter()).collect();
    for (got, want) in words.iter().zip(&original_words) {
        assert_eq!(
            (got.w.as_str(), got.start_sec, got.end_sec),
            (want.w.as_str(), want.start_sec, want.end_sec)
        );
    }
    assert_eq!(words.len(), original_words.len());
    assert_eq!(units[1].utterance, 1);
}
#[test]
fn duplicate_windows_do_not_inflate_minority_coverage() {
    let u = super::super::tests::utt(0.0, 8.0, "", 0, "one two three four");
    let mut utterances = vec![u];
    let mut units = vec![
        unit(0, 0.0, 7.0, 0),
        unit(0, 7.0, 8.0, 1),
        unit(0, 7.0, 8.0, 1),
        unit(0, 7.0, 8.0, 1),
    ];
    let mut labels = HashMap::from([(0, 0), (1, 1), (2, 1), (3, 1)]);
    split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut labels,
        SpeechSupport::default(),
    );
    assert_eq!(utterances.len(), 1, "repeated 1s is only 12.5% coverage");
    let coverage = label_durations(&units, &labels, SpeechSupport::default());
    assert_eq!(coverage[&(0, 1)], 1.0);
}

#[test]
fn quarter_coverage_under_one_second_does_not_split() {
    let mut utterances = vec![super::super::tests::utt(0.0, 3.0, "", 0, "one two")];
    let mut units = vec![unit(0, 0.0, 2.2, 0), unit(0, 2.2, 3.0, 1)];
    let mut labels = HashMap::from([(0, 0), (1, 1)]);
    split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut labels,
        SpeechSupport::default(),
    );
    assert_eq!(utterances.len(), 1);
}

#[test]
fn close_embeddings_block_cut_between_differently_labeled_runs() {
    let mut utterances = vec![super::super::tests::utt(
        0.0,
        8.0,
        "",
        0,
        "one two three four",
    )];
    let mut units = vec![unit(0, 0.0, 6.0, 0), unit(0, 6.0, 8.0, 1)];
    units[1].embedding = vec![0.9, (1.0 - 0.9_f64.powi(2)).sqrt()];
    let mut labels = HashMap::from([(0, 0), (1, 1)]);
    split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut labels,
        SpeechSupport::default(),
    );
    assert_eq!(
        utterances.len(),
        1,
        "run centroid cosine 0.9 is the same voice despite different labels"
    );
}

#[test]
fn distant_embeddings_keep_cut_between_runs() {
    let mut utterances = vec![super::super::tests::utt(
        0.0,
        8.0,
        "",
        0,
        "one two three four",
    )];
    let mut units = vec![unit(0, 0.0, 6.0, 0), unit(0, 6.0, 8.0, 1)];
    units[1].embedding = vec![0.2, (1.0 - 0.2_f64.powi(2)).sqrt()];
    let mut labels = HashMap::from([(0, 0), (1, 1)]);
    split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut labels,
        SpeechSupport::default(),
    );
    assert_eq!(
        utterances.len(),
        2,
        "run centroid cosine 0.2 is a real speaker change"
    );
}

#[test]
fn valid_cluster_at_both_minima_survives() {
    let units = vec![unit(0, 0.0, 28.5, 0), unit(1, 28.5, 30.0, 1)];
    let labels = reject_small_clusters(&units, &[0, 1], vec![0, 1], &[4.0, 1.5], &[], None);
    assert_ne!(labels.labels[0], labels.labels[1]);
}

#[test]
fn rejection_preserves_cannot_link() {
    let units = vec![unit(0, 0.0, 60.0, 0), unit(1, 60.0, 62.9, 1)];
    let labels = reject_small_clusters(&units, &[0, 1], vec![0, 1], &[4.0, 2.9], &[(0, 1)], None);
    assert_ne!(labels.labels[0], labels.labels[1]);
}
#[test]
fn splits_textless_vad_turns_for_diarization() {
    let mut utterances = vec![super::super::tests::utt(0.0, 8.0, "", 0, "")];
    let mut units = vec![unit(0, 0.0, 6.0, 0), unit(0, 6.0, 8.0, 1)];
    let mut labels = HashMap::from([(0, 0), (1, 1)]);
    split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut labels,
        SpeechSupport::default(),
    );
    assert_eq!(utterances.len(), 2);
    assert_eq!((utterances[0].end, utterances[1].start), (6.0, 6.0));
    assert!(utterances
        .iter()
        .all(|u| u.text.is_empty() && u.words.is_empty()));
    assert_eq!(units[1].utterance, 1);
}
struct SplitTurns {
    utterances: Vec<Utterance>,
    embs: Vec<Option<Vec<f64>>>,
    durs: Vec<f64>,
    labels: HashMap<usize, usize>,
    protected: HashMap<usize, usize>,
}

fn split_three_turns() -> SplitTurns {
    let mut utterances = vec![super::super::tests::utt(0.0, 8.0, "", 0, "")];
    let mut units = vec![
        unit(0, 0.0, 3.0, 0),
        unit(0, 3.0, 5.0, 1),
        unit(0, 5.0, 8.0, 0),
    ];
    let mut labels = HashMap::from([(0, 0), (1, 1), (2, 0)]);
    let protected = split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut labels,
        SpeechSupport::default(),
    );
    assert_eq!(utterances.len(), 3);
    let embs = vec![
        Some(vec![1.0, 0.0]),
        Some(vec![0.65, (1.0_f64 - 0.65_f64.powi(2)).sqrt()]),
        Some(vec![1.0, 0.0]),
    ];
    SplitTurns {
        utterances,
        embs,
        durs: vec![3.0, 2.0, 3.0],
        labels: HashMap::from([(0, 0), (1, 1), (2, 0)]),
        protected,
    }
}

#[test]
fn significant_coverage_survives_mono_smoothing() {
    let SplitTurns {
        utterances,
        embs,
        durs,
        mut labels,
        protected,
    } = split_three_turns();
    super::super::smooth_labels_mono(&utterances, &embs, &durs, &[1.0; 3], &mut labels);
    restore_protected(&mut labels, &protected);
    assert_ne!(
        labels[&0], labels[&1],
        "25% acoustically attributed middle turn must survive smoothing"
    );
}

#[test]
fn significant_coverage_survives_single_local_segmentation() {
    let SplitTurns {
        utterances,
        embs,
        durs,
        mut labels,
        protected,
    } = split_three_turns();
    let segmentation = Segmentation {
        frames: vec![1; 500],
        windows: vec![mr_ml::onnx::segmentation::SegmentationWindow {
            start_sample: 0,
            classes: vec![1; 500],
        }],
    };
    super::super::super::speaker_assignment::apply(
        &segmentation,
        &utterances,
        &embs,
        &durs,
        &[1.0; 3],
        &mut labels,
        2,
    );
    restore_protected(&mut labels, &protected);
    assert_ne!(
        labels[&0], labels[&1],
        "single local class must not erase significant acoustic coverage"
    );
}

#[test]
fn sampled_short_majority_does_not_inflate_minority_share() {
    let mut units = Vec::new();
    let mut labels = Vec::new();
    for utterance in 0..26 {
        let start = utterance as f64 * 3.0;
        let (duration, label) = if utterance < 25 { (2.4, 0) } else { (2.9, 1) };
        for (from, to) in super::super::acoustic_windows::bounds(start, start + duration) {
            if to - from >= super::super::CORE_MIN_DUR {
                units.push(unit(utterance, from, to, label));
                labels.push(label);
            }
        }
    }
    let core: Vec<_> = (0..units.len()).collect();
    let weights: Vec<_> = units.iter().map(|unit| unit.dur).collect();
    let labels = reject_small_clusters(&units, &core, labels, &weights, &[], None);
    assert!(
        labels.labels.iter().all(|label| *label == labels.labels[0]),
        "2.9s is below 5% of 60s+2.9s speech with actual short-window sampling"
    );
}

#[test]
fn recovers_supported_speakers_after_outlier_consumed_auto_slot() {
    let mut units = vec![
        unit(0, 0.0, 10.0, 0),
        unit(1, 10.0, 20.0, 1),
        unit(2, 20.0, 21.0, 0),
    ];
    units[2].embedding = vec![-std::f64::consts::FRAC_1_SQRT_2; 2];
    let validated = reject_small_clusters(
        &units,
        &[0, 1, 2],
        vec![0, 0, 1],
        &[4.0, 4.0, 1.0],
        &[],
        None,
    );
    assert_eq!(validated.core_units, vec![0, 1]);
    assert_ne!(validated.labels[0], validated.labels[1]);
}

#[test]
fn remaps_cannot_link_after_removing_earlier_outlier() {
    let mut units = vec![
        unit(0, 0.0, 1.0, 0),
        unit(1, 1.0, 11.0, 0),
        unit(2, 11.0, 12.0, 1),
    ];
    units[0].embedding = vec![-std::f64::consts::FRAC_1_SQRT_2; 2];
    let validated = reject_small_clusters(
        &units,
        &[0, 1, 2],
        vec![0, 1, 2],
        &[1.0, 4.0, 1.0],
        &[(1, 2)],
        None,
    );
    assert_eq!(validated.core_units, vec![1, 2]);
    assert_ne!(validated.labels[0], validated.labels[1]);
}

#[test]
fn all_short_clusters_keep_largest_support_deterministically() {
    let units = vec![
        unit(0, 0.0, 0.5, 0),
        unit(1, 0.5, 1.5, 1),
        unit(2, 1.5, 2.5, 0),
    ];
    let validated = reject_small_clusters(
        &units,
        &[0, 1, 2],
        vec![0, 1, 2],
        &[0.5, 1.0, 1.0],
        &[],
        None,
    );
    assert_eq!(validated.core_units, vec![1]);
    assert_eq!(validated.labels, vec![0]);
}

#[test]
fn configured_minimum_prevents_unsupported_rejection() {
    let units = vec![unit(0, 0.0, 10.0, 0), unit(1, 10.0, 11.0, 1)];
    let validated = super::reject_small_clusters(
        &units,
        &[0, 1],
        vec![0, 1],
        &[4.0, 1.0],
        &[],
        SpeechSupport::default(),
        2..=8,
    );
    assert_eq!(validated.core_units, vec![0, 1]);
    assert_ne!(validated.labels[0], validated.labels[1]);
}

#[test]
fn wall_time_does_not_validate_cluster_without_enough_vad_speech() {
    let units = vec![unit(0, 0.0, 10.0, 0), unit(1, 10.0, 12.0, 1)];
    let mut vad = vec![1.0; 375];
    vad[343..].fill(0.0);
    let validated = super::reject_small_clusters(
        &units,
        &[0, 1],
        vec![0, 1],
        &[4.0, 2.0],
        &[],
        SpeechSupport {
            vad_probs: Some(&vad),
            segmentation: None,
        },
        1..=8,
    );
    assert_eq!(
        validated.core_units,
        vec![0],
        "two-second window has only 0.976s of speech"
    );
}
