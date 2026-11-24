use super::*;
use mr_ml::cluster::agglomerative_weighted;
use mr_ml::onnx::segmentation::{Segmentation, SegmentationWindow};
use mr_ml::onnx::vad::VAD_WINDOW;

fn time(frame: usize) -> f64 {
    (frame * FRAME_SAMPLES) as f64 / 16000.0
}

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

fn segmentation(classes: Vec<u8>) -> Segmentation {
    Segmentation {
        frames: vec![],
        windows: vec![SegmentationWindow {
            start_sample: 0,
            classes,
        }],
    }
}

fn links(segmentation: &Segmentation, units: &[AcoustUnit]) -> Vec<(usize, usize)> {
    let positions = (0..units.len()).map(|index| (index, index)).collect();
    cannot_link_units(
        units,
        &positions,
        SpeechSupport {
            segmentation: Some(segmentation),
            vad_probs: None,
        },
        &[],
    )
}

fn links_with_vectors(segmentation: &Segmentation, units: &[AcoustUnit]) -> Vec<(usize, usize)> {
    let positions = (0..units.len()).map(|index| (index, index)).collect();
    let vectors: Vec<Vec<f64>> = units.iter().map(|unit| unit.embedding.clone()).collect();
    cannot_link_units(
        units,
        &positions,
        SpeechSupport {
            segmentation: Some(segmentation),
            vad_probs: None,
        },
        &vectors,
    )
}

#[test]
fn contradictory_pair_is_dropped_only_with_confident_local_label() {
    // Один голос, разведённый PyAnnote на два локальных класса: в пограничной
    // зоне (0.65..0.75) уверенная метка (purity 1.0) означает ошибку меток,
    // а размытая (purity 0.7) сохраняет ограничение.
    let first = 100;
    let second = 100;
    let scene = |second_class_frames: usize| {
        let mut classes = vec![1u8; first];
        classes.extend(std::iter::repeat_n(2u8, second_class_frames));
        classes.extend(std::iter::repeat_n(3u8, second - second_class_frames));
        segmentation(classes)
    };
    let pair = [unit(0, time(0), time(100)), {
        let mut u = unit(1, time(100), time(200));
        u.embedding = vec![0.70, (1.0 - 0.70_f64 * 0.70).sqrt()];
        u
    }];
    let confident = links_with_vectors(&scene(second), &pair);
    assert!(
        confident.is_empty(),
        "purity 1.0 + sim 0.70 — это один голос, а не перекрытие: {confident:?}"
    );
    let blurry = links_with_vectors(&scene(70), &pair);
    assert_eq!(
        blurry,
        vec![(0, 1)],
        "purity 0.7 оставляет ограничение в зоне 0.65..0.75"
    );
}

#[test]
fn high_similarity_pair_is_dropped_even_with_blurry_local_label() {
    let scene = |second_class_frames: usize| {
        let mut classes = vec![1u8; 100];
        classes.extend(std::iter::repeat_n(2u8, second_class_frames));
        classes.extend(std::iter::repeat_n(3u8, 100 - second_class_frames));
        segmentation(classes)
    };
    let pair_with_sim = |sim: f64| {
        [unit(0, time(0), time(100)), {
            let mut u = unit(1, time(100), time(200));
            u.embedding = vec![sim, (1.0 - sim * sim).sqrt()];
            u
        }]
    };
    let high_sim = links_with_vectors(&scene(65), &pair_with_sim(0.76));
    assert!(
        high_sim.is_empty(),
        "sim >= 0.75 сбрасывается даже при min_purity 0.65: {high_sim:?}"
    );
    let overlap_pair = links_with_vectors(&scene(68), &pair_with_sim(0.677));
    assert_eq!(
        overlap_pair,
        vec![(0, 1)],
        "калибровочная пара перекрытия (sim 0.677, min_purity 0.68) сохраняется"
    );
}

#[test]
fn different_turns_with_identical_embeddings_stay_apart_even_with_fixed_k_one() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 100]);
    let segmentation = segmentation(classes);
    let units = [unit(0, time(0), time(100)), unit(1, time(100), time(200))];
    let constraints = links(&segmentation, &units);
    assert_eq!(constraints, vec![(0, 1)]);
    let embeddings: Vec<_> = units.iter().map(|unit| unit.embedding.clone()).collect();
    let labels = agglomerative_weighted(&embeddings, &[1.0, 1.0], &constraints, 0.55, Some(1));
    assert_ne!(labels[0], labels[1]);
}

#[test]
fn local_ids_from_different_windows_do_not_imply_different_speakers() {
    let segmentation = Segmentation {
        frames: vec![],
        windows: vec![
            SegmentationWindow {
                start_sample: 0,
                classes: vec![1; 100],
            },
            SegmentationWindow {
                start_sample: 160_000,
                classes: vec![2; 100],
            },
        ],
    };
    let units = [unit(0, time(0), time(100)), unit(1, 10.0, 10.0 + time(100))];
    assert!(links(&segmentation, &units).is_empty());
}

#[test]
fn short_window_edge_support_does_not_force_a_speaker() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 20]);
    let segmentation = segmentation(classes);
    let units = [unit(0, time(0), time(100)), unit(1, time(100), time(200))];
    assert!(links(&segmentation, &units).is_empty());
}

#[test]
fn mixed_unit_below_existing_purity_does_not_force_a_speaker() {
    let mut classes = vec![1; 110];
    classes.extend(vec![2; 100]);
    classes.extend(vec![3; 100]);
    let segmentation = segmentation(classes);
    let units = [unit(0, time(0), time(210)), unit(1, time(210), time(310))];
    assert!(links(&segmentation, &units).is_empty());
}

#[test]
fn absent_vad_support_does_not_create_a_constraint() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 100]);
    let segmentation = segmentation(classes);
    let units = [unit(0, time(0), time(100)), unit(1, time(100), time(200))];
    let mut vad = vec![1.0; (FRAME_SAMPLES * 200).div_ceil(VAD_WINDOW)];
    vad[(FRAME_SAMPLES * 100) / VAD_WINDOW..].fill(0.0);
    let constraints = cannot_link_units(
        &units,
        &HashMap::from([(0, 0), (1, 1)]),
        SpeechSupport {
            segmentation: Some(&segmentation),
            vad_probs: Some(&vad),
        },
        &[],
    );
    assert!(constraints.is_empty());
}

#[test]
fn global_overlap_does_not_create_a_constraint() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 100]);
    let mut segmentation = segmentation(classes);
    segmentation.frames = vec![1; 100];
    segmentation.frames.extend(vec![4; 100]);
    let units = [unit(0, time(0), time(100)), unit(1, time(100), time(200))];
    assert!(links(&segmentation, &units).is_empty());
}

#[test]
fn purity_uses_only_the_shared_clean_speech_support() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 100]);
    classes.extend(vec![3; 100]);
    let segmentation = segmentation(classes);
    let units = [unit(0, time(0), time(200)), unit(1, time(200), time(300))];
    let mut vad = vec![1.0; (FRAME_SAMPLES * 300).div_ceil(VAD_WINDOW)];
    let from = (FRAME_SAMPLES * 100) / VAD_WINDOW;
    let to = (FRAME_SAMPLES * 200).div_ceil(VAD_WINDOW);
    vad[from..to].fill(0.0);
    assert_eq!(
        cannot_link_units(
            &units,
            &HashMap::from([(0, 0), (1, 1)]),
            SpeechSupport {
                segmentation: Some(&segmentation),
                vad_probs: Some(&vad),
            },
            &[],
        ),
        vec![(0, 1)]
    );
}

#[test]
fn core_positions_are_used_instead_of_unit_indices() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 100]);
    let segmentation = segmentation(classes);
    let units = [
        unit(0, time(0), time(100)),
        unit(1, time(0), time(200)),
        unit(2, time(100), time(200)),
    ];
    assert_eq!(
        cannot_link_units(
            &units,
            &HashMap::from([(0, 1), (2, 0)]),
            SpeechSupport {
                segmentation: Some(&segmentation),
                vad_probs: None,
            },
            &[],
        ),
        vec![(0, 1)]
    );
}

#[test]
fn fractional_frame_edges_do_not_inflate_purity() {
    let mut classes = vec![1; 60];
    classes.extend(vec![2; 40]);
    classes.extend(vec![3; 100]);
    let segmentation = segmentation(classes);
    let units = [
        unit(0, time(0) + 0.9 * time(1), time(100)),
        unit(1, time(100), time(200)),
    ];
    assert!(links(&segmentation, &units).is_empty());
}

#[test]
fn fractional_frame_edges_do_not_inflate_minimum_support() {
    let mut classes = vec![1; 60];
    classes.extend(vec![2; 100]);
    let segmentation = segmentation(classes);
    let units = [
        unit(0, time(0) + 0.9 * time(1), time(60)),
        unit(1, time(60), time(160)),
    ];
    assert!(links(&segmentation, &units).is_empty());
}

#[test]
fn duplicate_window_evidence_produces_one_pair() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 100]);
    let mut segmentation = segmentation(classes);
    segmentation.windows.push(segmentation.windows[0].clone());
    let units = [unit(0, time(0), time(100)), unit(1, time(100), time(200))];
    assert_eq!(links(&segmentation, &units), vec![(0, 1)]);
}

#[test]
fn dropping_a_core_unit_keeps_remaining_pairs_unchanged() {
    let mut classes = vec![1; 100];
    classes.extend(vec![2; 100]);
    classes.extend(vec![3; 100]);
    let segmentation = segmentation(classes);
    let units = [
        unit(0, time(0), time(100)),
        unit(1, time(100), time(200)),
        unit(2, time(200), time(300)),
    ];
    let full = links(&segmentation, &units);
    assert_eq!(full, vec![(0, 1), (0, 2), (1, 2)]);
    let support = SpeechSupport {
        segmentation: Some(&segmentation),
        vad_probs: None,
    };
    let without_middle = cannot_link_units(&units, &HashMap::from([(0, 0), (2, 1)]), support, &[]);
    assert_eq!(without_middle, vec![(0, 1)]);
}

#[test]
fn pairs_above_sim_keeps_only_pairs_contradicting_embedding_closeness() {
    let vecs = vec![vec![1.0, 0.0], vec![0.9, 0.1], vec![0.0, 1.0]];
    let pairs = vec![(0usize, 1usize), (0, 2), (1, 2)];
    assert_eq!(pairs_above_sim(&vecs, &pairs, 0.5), vec![(0, 1)]);
    assert_eq!(pairs_above_sim(&vecs, &pairs, 0.999), vec![]);
    assert_eq!(pairs_above_sim(&[], &pairs, 0.5), vec![]);
}
