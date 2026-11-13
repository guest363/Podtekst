use std::collections::HashMap;

use crate::cosine_similarity;

struct DistanceMatrix {
    n: usize,
    values: Vec<f64>,
}

impl DistanceMatrix {
    fn new(embeddings: &[Vec<f64>]) -> Self {
        let n = embeddings.len();
        let mut values = vec![0.0; n * (n - 1) / 2];
        for i in 0..n {
            for j in (i + 1)..n {
                let index = Self::index(n, i, j);
                values[index] = 1.0 - cosine_similarity(&embeddings[i], &embeddings[j]);
            }
        }
        Self { n, values }
    }

    fn index(n: usize, i: usize, j: usize) -> usize {
        let (i, j) = if i < j { (i, j) } else { (j, i) };
        i * (2 * n - i - 1) / 2 + j - i - 1
    }

    fn get(&self, i: usize, j: usize) -> f64 {
        self.values[Self::index(self.n, i, j)]
    }

    fn set(&mut self, i: usize, j: usize, value: f64) {
        let index = Self::index(self.n, i, j);
        self.values[index] = value;
    }
}

const CANNOT_LINK_DISTANCE: f64 = 10.0;
/// Минимальный абсолютный разрыв между высотами слияний, при котором
/// автоматический выбор k считается уверенным. shortcut: 0.04; проверено на
/// VoxConverse и AMI через tools/diar_eval, см. docs/testing/BENCHMARK-REPORT.md.
const MIN_AUTO_K_GAP: f64 = 0.04;

pub fn agglomerative(
    embeddings: &[Vec<f64>],
    distance_threshold: f64,
    n_clusters: Option<usize>,
) -> Vec<usize> {
    let weights = vec![1.0; embeddings.len()];
    agglomerative_weighted(embeddings, &weights, &[], distance_threshold, n_clusters)
}

/// Nearest-neighbor chain строит иерархию один раз; выбор k лишь применяет
/// её к дереву, поэтому порог, fixed-k и разрывы не пересчитывают матрицу.
fn chain_merges(
    embeddings: &[Vec<f64>],
    weights: &[f64],
    cannot_link: &[(usize, usize)],
) -> Vec<(f64, usize, usize)> {
    let n = embeddings.len();
    if n < 2 {
        return vec![];
    }
    let mut distances = DistanceMatrix::new(embeddings);
    let mut active = vec![true; n];
    let mut sizes: Vec<f64> = (0..n)
        .map(|i| weights.get(i).copied().unwrap_or(1.0).max(1e-6))
        .collect();
    let mut blocked = vec![vec![false; n]; n];
    for &(i, j) in cannot_link {
        if i >= n || j >= n || i == j {
            continue;
        }
        blocked[i][j] = true;
        blocked[j][i] = true;
        distances.set(i, j, CANNOT_LINK_DISTANCE);
    }
    let mut active_count = n;
    let mut chain = Vec::with_capacity(n);
    let mut merges = Vec::with_capacity(n - 1);

    while active_count > 1 {
        if chain.is_empty() {
            chain.push(active.iter().position(|&x| x).unwrap());
        }
        let a = *chain.last().unwrap();
        let b = (0..n)
            .filter(|&i| active[i] && i != a)
            .min_by(|&i, &j| distances.get(a, i).total_cmp(&distances.get(a, j)))
            .unwrap();
        if chain.len() >= 2 && chain[chain.len() - 2] == b {
            let merge_distance = distances.get(a, b);
            chain.pop();
            chain.pop();
            // Average linkage: шумные короткие реплики одного голоса не
            // разваливают кластер из-за одной дальней пары, как при complete.
            let (wa, wb) = (sizes[a], sizes[b]);
            for (c, &is_active) in active.iter().enumerate() {
                if is_active && c != a && c != b {
                    let d = (distances.get(a, c) * wa + distances.get(b, c) * wb) / (wa + wb);
                    let d = if blocked[a][c] || blocked[b][c] {
                        CANNOT_LINK_DISTANCE
                    } else {
                        d
                    };
                    distances.set(a, c, d);
                    blocked[a][c] = blocked[a][c] || blocked[b][c];
                    blocked[c][a] = blocked[a][c];
                }
            }
            sizes[a] += sizes[b];
            active[b] = false;
            active_count -= 1;
            merges.push((merge_distance, a, b));
        } else {
            chain.push(b);
        }
    }

    // Цепочка даёт иную последовательность слияний: fixed-k и порог
    // применяем к иерархии только после сортировки по их высоте.
    merges.sort_by(|a, b| a.0.total_cmp(&b.0));
    merges
}

fn labels_with_limit(
    n: usize,
    merges: &[(f64, usize, usize)],
    target_k: usize,
    threshold: f64,
) -> Vec<usize> {
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }

    let mut parent: Vec<usize> = (0..n).collect();
    let mut cluster_count = n;
    for &(distance, a, b) in merges {
        if cluster_count <= target_k || distance > threshold {
            break;
        }
        if distance >= CANNOT_LINK_DISTANCE {
            log::warn!(
                target: "cluster",
                requested_k = target_k,
                retained_k = cluster_count;
                "ev=cluster.constraint_limit"
            );
            break;
        }
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[rb] = ra;
            cluster_count -= 1;
        }
    }
    let mut labels = vec![0usize; n];
    for (i, label) in labels.iter_mut().enumerate() {
        *label = find(&mut parent, i);
    }
    let mut remap = HashMap::new();
    let mut next = 0usize;
    for l in labels.iter_mut() {
        let e = remap.entry(*l).or_insert_with(|| {
            let v = next;
            next += 1;
            v
        });
        *l = *e;
    }
    labels
}

pub fn agglomerative_weighted(
    embeddings: &[Vec<f64>],
    weights: &[f64],
    cannot_link: &[(usize, usize)],
    distance_threshold: f64,
    n_clusters: Option<usize>,
) -> Vec<usize> {
    let n = embeddings.len();
    if n == 0 {
        return vec![];
    }
    let merges = chain_merges(embeddings, weights, cannot_link);
    let target_k = n_clusters.map(|k| k.clamp(1, n)).unwrap_or(1);
    let threshold = if n_clusters.is_some() {
        f64::INFINITY
    } else {
        distance_threshold
    };
    labels_with_limit(n, &merges, target_k, threshold)
}

/// Автоматическое k по максимальному разрыву в высотах слияний: аналог
/// eigen-gap для spectral clustering, но на уже готовом дереве AHC.
/// Без уверенного разрыва работает как обычный порог расстояния.
pub fn agglomerative_gap(
    embeddings: &[Vec<f64>],
    weights: &[f64],
    cannot_link: &[(usize, usize)],
    distance_threshold: f64,
    min_clusters: usize,
    max_clusters: usize,
) -> Vec<usize> {
    let n = embeddings.len();
    if n == 0 {
        return vec![];
    }
    let merges = chain_merges(embeddings, weights, cannot_link);
    let top_merges: Vec<(f64, usize, usize)> = merges.iter().rev().take(10).copied().collect();
    log::debug!(
        target: "cluster",
        n = n,
        merges_tail = format!("{top_merges:?}").as_str();
        "ev=cluster.merges_tail"
    );
    let min_k = min_clusters.clamp(1, n);
    let max_k = max_clusters.clamp(min_k, n);
    // k=1 — это «один голос», его даёт fallback по порогу расстояния, а не
    // разрыв в дереве; иначе искусственная высота cannot-link и шумовая пара
    // тянут k вниз, вплоть до одного кластера после слияний.
    let gap_floor = if min_k <= 1 { 2 } else { min_k };
    let mut best: Option<(f64, usize)> = None;
    for i in 0..merges.len().saturating_sub(1) {
        let k = n - (i + 1);
        if k < gap_floor || k > max_k {
            continue;
        }
        // shortcut: слияния >= 2.0 вызваны искусственным CANNOT_LINK_DISTANCE (10.0),
        // искусственный скачок не является акустическим разрывом в дендрограмме.
        if merges[i + 1].0 >= 2.0 {
            continue;
        }
        let gap = merges[i + 1].0 - merges[i].0;
        if best.is_none_or(|(best_gap, _)| gap > best_gap) {
            best = Some((gap, k));
        }
    }
    if let Some((gap, k)) = best.filter(|(gap, _)| *gap >= MIN_AUTO_K_GAP) {
        log::debug!(
            target: "cluster",
            k = k,
            gap = gap;
            "ev=cluster.auto_k"
        );
        return labels_with_limit(n, &merges, k, f64::INFINITY);
    }
    log::debug!(
        target: "cluster",
        n = n,
        min_k = min_k,
        max_k = max_k;
        "ev=cluster.auto_k_fallback"
    );
    let labels = labels_with_limit(n, &merges, min_k, distance_threshold);
    let mut unique: Vec<usize> = labels.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() > max_k {
        labels_with_limit(n, &merges, max_k, f64::INFINITY)
    } else {
        labels
    }
}

fn link_blocked(labels: &[usize], cannot_link: &[(usize, usize)], a: usize, b: usize) -> bool {
    cannot_link
        .iter()
        .any(|&(i, j)| match (labels.get(i), labels.get(j)) {
            (Some(&left), Some(&right)) => (left == a && right == b) || (left == b && right == a),
            _ => false,
        })
}

/// Сливает кластеры, чьи центроиды ближе порога: AHC может развести один
/// голос на два из-за акустических условий. Ограничения cannot-link
/// сохраняются, чтобы не склеить двух локальных спикеров одной реплики.
pub fn merge_by_centroid_cosine(
    embeddings: &[Vec<f64>],
    weights: &[f64],
    labels: &mut [usize],
    cannot_link: &[(usize, usize)],
    threshold: f64,
    min_keep: usize,
) {
    let n = embeddings.len().min(labels.len());
    if n == 0 {
        return;
    }
    let mut unique: Vec<usize> = labels[..n].to_vec();
    unique.sort_unstable();
    unique.dedup();
    let min_keep = min_keep.clamp(1, unique.len());
    while unique.len() > min_keep {
        let centroids: Vec<(usize, Vec<f64>)> = unique
            .iter()
            .map(|&label| {
                let members: Vec<usize> = (0..n).filter(|&i| labels[i] == label).collect();
                let vecs: Vec<Vec<f64>> = members.iter().map(|&i| embeddings[i].clone()).collect();
                let ws: Vec<f64> = members
                    .iter()
                    .map(|&i| weights.get(i).copied().unwrap_or(1.0).max(1e-6))
                    .collect();
                (label, crate::weighted_centroid(&vecs, &ws))
            })
            .collect();
        let mut best: Option<(usize, usize, f64)> = None;
        for i in 0..centroids.len() {
            for j in (i + 1)..centroids.len() {
                let (left, right) = (centroids[i].0, centroids[j].0);
                if centroids[i].1.is_empty()
                    || centroids[j].1.is_empty()
                    || link_blocked(labels, cannot_link, left, right)
                {
                    continue;
                }
                let similarity = crate::cosine_similarity(&centroids[i].1, &centroids[j].1);
                if best.is_none_or(|(_, _, best_sim)| similarity > best_sim) {
                    best = Some((left, right, similarity));
                }
            }
        }
        let Some((left, right, similarity)) = best else {
            break;
        };
        if similarity < threshold {
            break;
        }
        log::debug!(
            target: "cluster",
            from = right,
            into = left,
            similarity = similarity;
            "ev=cluster.merge_centroid"
        );
        for label in labels[..n].iter_mut() {
            if *label == right {
                *label = left;
            }
        }
        unique.retain(|&label| label != right);
    }
}

/// Силуэт каждого фрагмента по косинусному расстоянию, диапазон -1..1.
/// Одиночный кластер и вырожденная нулевая дистанция дают conventional 0.
pub fn silhouette_scores(embeddings: &[Vec<f64>], labels: &[usize]) -> Vec<f64> {
    let n = embeddings.len();
    if labels.len() != n {
        return vec![];
    }
    if n < 2 {
        return vec![0.0; n];
    }
    let distances = DistanceMatrix::new(embeddings);
    let mut scores = vec![0.0; n];
    for i in 0..n {
        let mut sums: HashMap<usize, (f64, usize)> = HashMap::new();
        for (j, &label) in labels.iter().enumerate() {
            if i == j {
                continue;
            }
            let entry = sums.entry(label).or_default();
            entry.0 += distances.get(i, j);
            entry.1 += 1;
        }
        let Some(&(own_sum, own_count)) = sums.get(&labels[i]) else {
            continue;
        };
        if own_count == 0 || sums.len() < 2 {
            continue;
        }
        let a = own_sum / own_count as f64;
        let b = sums
            .iter()
            .filter(|(label, _)| **label != labels[i])
            .map(|(_, &(sum, count))| sum / count as f64)
            .fold(f64::INFINITY, f64::min);
        let denominator = a.max(b);
        if denominator > f64::EPSILON {
            scores[i] = ((b - a) / denominator).clamp(-1.0, 1.0);
        }
    }
    scores
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn unit(dim: usize, idx: usize) -> Vec<f64> {
        let mut v = vec![0.0; dim];
        v[idx % dim] = 1.0;
        v
    }

    #[test]
    fn separates_two_speakers() {
        let mut embs = vec![];
        for _ in 0..3 {
            embs.push(unit(8, 0));
        }
        for _ in 0..3 {
            embs.push(unit(8, 1));
        }
        let labels = agglomerative(&embs, 0.55, None);
        assert_eq!(labels[0], labels[2]);
        assert_eq!(labels[3], labels[5]);
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn fixed_k() {
        let embs = vec![unit(4, 0), unit(4, 1), unit(4, 2)];
        let labels = agglomerative(&embs, 0.55, Some(2));
        let uniq: HashSet<usize> = labels.iter().copied().collect();
        assert_eq!(uniq.len(), 2);
    }

    #[allow(clippy::needless_range_loop)]
    fn naive_average(embeddings: &[Vec<f64>], k: usize) -> Vec<usize> {
        let n = embeddings.len();
        let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
        while clusters.len() > k {
            let m = clusters.len();
            let (mut a, mut b, mut best) = (0, 1, f64::INFINITY);
            for i in 0..m {
                for j in (i + 1)..m {
                    let mut sum = 0.0;
                    for &x in &clusters[i] {
                        for &y in &clusters[j] {
                            sum += 1.0 - cosine_similarity(&embeddings[x], &embeddings[y]);
                        }
                    }
                    let distance = sum / (clusters[i].len() * clusters[j].len()) as f64;
                    if distance < best {
                        (a, b, best) = (i, j, distance);
                    }
                }
            }
            let members = clusters.remove(b);
            clusters[a].extend(members);
        }
        let mut labels = vec![0; n];
        for (label, members) in clusters.iter().enumerate() {
            for &i in members {
                labels[i] = label;
            }
        }
        labels
    }

    #[test]
    fn chain_matches_naive_average_linkage() {
        let embs = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.95, 0.1, 0.05],
            vec![0.8, 0.3, 0.1],
            vec![0.1, 1.0, 0.2],
            vec![0.2, 0.85, 0.1],
            vec![0.3, 0.75, 0.2],
            vec![0.6, 0.4, 0.3],
        ];
        for k in 1..=embs.len() {
            let actual = agglomerative(&embs, f64::INFINITY, Some(k));
            let expected = naive_average(&embs, k);
            for i in 0..embs.len() {
                for j in 0..embs.len() {
                    assert_eq!(
                        actual[i] == actual[j],
                        expected[i] == expected[j],
                        "k={k} i={i} j={j} actual={actual:?} expected={expected:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn average_linkage_keeps_noisy_fragments_of_one_voice_together() {
        // ±35° вокруг центра: complete linkage разрывает кластер по паре
        // с расстоянием 0.66, average — нет (после слияния 0.42).
        let at = |deg: f64| vec![deg.to_radians().cos(), deg.to_radians().sin()];
        let embs = vec![
            at(0.0),
            at(35.0),
            at(-35.0),
            at(180.0),
            at(145.0),
            at(215.0),
        ];
        let labels = agglomerative(&embs, 0.52, None);
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[0], labels[2]);
        assert_eq!(labels[3], labels[4]);
        assert_eq!(labels[3], labels[5]);
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn cannot_link_prevents_merge_of_similar_embeddings() {
        let embs = vec![unit(4, 0), unit(4, 0)];
        let labels = agglomerative_weighted(&embs, &[1.0, 1.0], &[(0, 1)], 0.55, None);
        assert_ne!(labels[0], labels[1]);
        let free = agglomerative(&embs, 0.55, None);
        assert_eq!(free[0], free[1]);
    }

    #[test]
    fn weighted_linkage_lets_long_segment_dominate() {
        let embs = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.7, 0.7]];
        let low = agglomerative_weighted(&embs, &[1.0, 1.0, 0.01], &[], 0.7, None);
        assert_ne!(low[0], low[1]);
        let high = agglomerative_weighted(&embs, &[1.0, 1.0, 10.0], &[], 0.7, None);
        assert_eq!(high[0], high[1]);
    }

    #[test]
    fn silhouette_rewards_compact_separated_clusters() {
        let embs = vec![unit(2, 0), unit(2, 0), unit(2, 1), unit(2, 1)];
        let scores = silhouette_scores(&embs, &[0, 0, 1, 1]);
        assert!(scores.iter().all(|&s| (s - 1.0).abs() < 1e-9));
        assert_eq!(silhouette_scores(&embs[..3], &[0, 1, 1])[0], 0.0);
        assert!(silhouette_scores(&embs, &[0, 0, 0, 0])
            .iter()
            .all(|&s| s == 0.0));
        assert!(silhouette_scores(&embs, &[0, 0]).is_empty());
    }

    fn unique_labels(labels: &[usize]) -> usize {
        let mut unique: Vec<usize> = labels.to_vec();
        unique.sort_unstable();
        unique.dedup();
        unique.len()
    }

    #[test]
    fn gap_selects_clear_two_voice_split() {
        let mut embs = vec![];
        for _ in 0..3 {
            embs.push(unit(8, 0));
        }
        for _ in 0..3 {
            embs.push(unit(8, 1));
        }
        let labels = agglomerative_gap(&embs, &[1.0; 6], &[], 0.52, 1, 6);
        assert_eq!(unique_labels(&labels), 2);
        assert_eq!(labels[0], labels[2]);
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn gap_falls_back_to_threshold_for_one_voice() {
        let at = |deg: f64| vec![deg.to_radians().cos(), deg.to_radians().sin()];
        let embs = vec![at(0.0), at(5.0), at(-5.0), at(3.0)];
        let labels = agglomerative_gap(&embs, &[1.0; 4], &[], 0.52, 1, 4);
        assert_eq!(unique_labels(&labels), 1);
    }

    #[test]
    fn gap_respects_max_clusters() {
        let embs = vec![
            unit(8, 0),
            unit(8, 0),
            unit(8, 1),
            unit(8, 1),
            unit(8, 2),
            unit(8, 2),
        ];
        let labels = agglomerative_gap(&embs, &[1.0; 6], &[], 0.52, 1, 2);
        assert_eq!(unique_labels(&labels), 2);
    }

    #[test]
    fn gap_clustering_keeps_cannot_link_apart() {
        let embs = vec![unit(4, 0); 4];
        let weights = [1.0; 4];
        let free = agglomerative_gap(&embs, &weights, &[], 0.52, 1, 4);
        assert_eq!(unique_labels(&free), 1);
        let blocked = agglomerative_gap(&embs, &weights, &[(0, 1)], 0.52, 1, 4);
        assert_ne!(blocked[0], blocked[1]);
    }

    #[test]
    fn gap_keeps_split_with_noise_instead_of_one_cluster() {
        let at = |deg: f64| vec![deg.to_radians().cos(), deg.to_radians().sin()];
        let embs = vec![
            at(0.0),
            at(2.0),
            at(-2.0),
            at(180.0),
            at(178.0),
            at(182.0),
            at(60.0),
        ];
        let labels = agglomerative_gap(&embs, &[1.0; 7], &[], 0.52, 1, 7);
        assert!(unique_labels(&labels) >= 2);
    }

    #[test]
    fn centroid_merge_joins_split_voice_but_keeps_distinct() {
        let embs = vec![
            vec![1.0, 0.0],
            vec![0.99, 0.1],
            vec![0.98, 0.05],
            vec![0.97, 0.12],
            vec![0.0, 1.0],
            vec![0.05, 0.99],
        ];
        let weights = vec![1.0; 6];
        let mut labels = vec![0, 0, 1, 1, 2, 2];
        merge_by_centroid_cosine(&embs, &weights, &mut labels, &[], 0.95, 1);
        assert_eq!(labels[0], labels[2]);
        assert_ne!(labels[0], labels[4]);
    }

    #[test]
    fn centroid_merge_respects_cannot_link_and_min_keep() {
        let identical = vec![unit(4, 0); 4];
        let weights = vec![1.0; 4];
        let mut labels = vec![0, 0, 1, 1];
        merge_by_centroid_cosine(&identical, &weights, &mut labels, &[(0, 2)], 0.9, 1);
        assert_ne!(labels[0], labels[2]);

        let embs = vec![unit(4, 0), unit(4, 0), unit(4, 1), unit(4, 1), unit(4, 0)];
        let mut labels = vec![0, 0, 1, 1, 2];
        merge_by_centroid_cosine(&embs, &[1.0; 5], &mut labels, &[], 0.9, 3);
        assert_eq!(unique_labels(&labels), 3);
    }
}
