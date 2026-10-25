//! Агломеративная кластеризация эмбеддингов (average linkage, косинусное расстояние).
//! Реализация UPGMA по схеме Ланса-Вильямса: время работы O(n²).

use std::collections::HashMap;

use crate::cosine_similarity;

/// Кластеризация векторов.
///
/// * `distance_threshold` — порог косинусного расстояния для слияния кластеров.
/// * `n_clusters` — точное число кластеров (min==max speakers), иначе автовыбор по порогу.
///
/// Возвращает метку кластера для каждого вектора.
#[allow(clippy::needless_range_loop)]
pub fn agglomerative(
    embeddings: &[Vec<f64>],
    distance_threshold: f64,
    n_clusters: Option<usize>,
) -> Vec<usize> {
    let n = embeddings.len();
    if n == 0 {
        return vec![];
    }
    // Матрица косинусных расстояний между кластерами (изначально 1 элемент = 1 кластер)
    let mut c_dist = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = 1.0 - cosine_similarity(&embeddings[i], &embeddings[j]);
            c_dist[i][j] = d;
            c_dist[j][i] = d;
        }
    }
    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let mut sizes: Vec<usize> = vec![1; n];

    let target_k = n_clusters.map(|k| k.max(1));
    while clusters.len() > target_k.unwrap_or(1) {
        let m = clusters.len();
        if m < 2 {
            break;
        }
        let (mut bi, mut bj, mut bd) = (0, 1, f64::INFINITY);
        for i in 0..m {
            for j in (i + 1)..m {
                let d = c_dist[i][j];
                if d < bd {
                    bd = d;
                    bi = i;
                    bj = j;
                }
            }
        }
        if target_k.is_none() && bd > distance_threshold {
            break;
        }

        // Рекуррентная формула Ланса-Вильямса для average linkage (UPGMA):
        // D(A ∪ B, C) = (|A| · D(A, C) + |B| · D(B, C)) / (|A| + |B|)
        let s_i = sizes[bi] as f64;
        let s_j = sizes[bj] as f64;
        let s_tot = s_i + s_j;
        for c in 0..m {
            if c != bi && c != bj {
                let d = (s_i * c_dist[bi][c] + s_j * c_dist[bj][c]) / s_tot;
                c_dist[bi][c] = d;
                c_dist[c][bi] = d;
            }
        }

        // Сливаем элементы кластера bj в bi
        let b = clusters.remove(bj);
        clusters[bi].extend(b);
        sizes[bi] += sizes.remove(bj);

        // Удаляем строку и столбец bj из матрицы расстояний
        c_dist.remove(bj);
        for row in c_dist.iter_mut() {
            row.remove(bj);
        }
    }

    let mut labels = vec![0usize; n];
    for (lid, members) in clusters.iter().enumerate() {
        for &m in members {
            labels[m] = lid;
        }
    }
    // Перенумеровываем по первому появлению для стабильности
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
}
