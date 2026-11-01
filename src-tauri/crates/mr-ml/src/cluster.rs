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

pub fn agglomerative(
    embeddings: &[Vec<f64>],
    distance_threshold: f64,
    n_clusters: Option<usize>,
) -> Vec<usize> {
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }

    let n = embeddings.len();
    if n == 0 {
        return vec![];
    }
    let mut distances = DistanceMatrix::new(embeddings);
    let mut active = vec![true; n];
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
            for (c, &is_active) in active.iter().enumerate() {
                if is_active && c != a && c != b {
                    let d = distances.get(a, c).max(distances.get(b, c));
                    distances.set(a, c, d);
                }
            }
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
    let target_k = n_clusters.map(|k| k.max(1)).unwrap_or(1);
    let mut parent: Vec<usize> = (0..n).collect();
    let mut cluster_count = n;
    for (distance, a, b) in merges {
        if cluster_count <= target_k || (n_clusters.is_none() && distance > distance_threshold) {
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
    fn naive_complete(embeddings: &[Vec<f64>], k: usize) -> Vec<usize> {
        let n = embeddings.len();
        let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
        let mut distances = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in (i + 1)..n {
                distances[i][j] = 1.0 - cosine_similarity(&embeddings[i], &embeddings[j]);
                distances[j][i] = distances[i][j];
            }
        }
        while clusters.len() > k {
            let m = clusters.len();
            let (mut a, mut b, mut best) = (0, 1, f64::INFINITY);
            for i in 0..m {
                for j in (i + 1)..m {
                    if distances[i][j] < best {
                        (a, b, best) = (i, j, distances[i][j]);
                    }
                }
            }
            for c in 0..m {
                if c != a && c != b {
                    distances[a][c] = distances[a][c].max(distances[b][c]);
                    distances[c][a] = distances[a][c];
                }
            }
            let members = clusters.remove(b);
            clusters[a].extend(members);
            distances.remove(b);
            for row in &mut distances {
                row.remove(b);
            }
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
    fn chain_matches_naive_complete_linkage() {
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
            let expected = naive_complete(&embs, k);
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
}
