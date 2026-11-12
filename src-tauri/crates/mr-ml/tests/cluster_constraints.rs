use mr_ml::cluster::{agglomerative_gap, agglomerative_weighted};

#[test]
fn fixed_count_never_merges_a_cannot_link_pair() {
    let vectors = vec![vec![1.0, 0.0], vec![0.999, 0.001], vec![0.0, 1.0]];
    let labels = agglomerative_weighted(&vectors, &[1.0; 3], &[(0, 1)], 0.52, Some(1));
    assert_ne!(labels[0], labels[1]);
}

#[test]
fn impossible_maximum_keeps_all_pairwise_constraints() {
    let vectors = vec![vec![1.0, 0.0]; 3];
    let constraints = [(0, 1), (0, 2), (1, 2)];
    let labels = agglomerative_gap(&vectors, &[1.0; 3], &constraints, 0.52, 1, 2);
    for (a, b) in constraints {
        assert_ne!(labels[a], labels[b]);
    }
}

#[test]
fn feasible_fixed_count_still_merges_unconstrained_duplicates() {
    let vectors = vec![vec![1.0, 0.0], vec![1.0, 0.0], vec![0.0, 1.0]];
    let labels = agglomerative_weighted(&vectors, &[1.0; 3], &[(0, 2)], 0.52, Some(2));
    assert_eq!(labels[0], labels[1]);
    assert_ne!(labels[0], labels[2]);
}
