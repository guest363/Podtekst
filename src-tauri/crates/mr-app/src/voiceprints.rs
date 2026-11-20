//! Единственная точка вызова голосовой математики из `mr-app`: HTTP-слой
//! не импортирует `mr-ml` напрямую (P3).

pub(crate) const EMBEDDING_SPACE: &str = mr_ml::EMBEDDING_SPACE;
pub(crate) const EMBEDDING_DIM: usize = mr_ml::EMBEDDING_DIM;
pub(crate) const MAX_PROTOTYPES: usize = mr_ml::MAX_PROTOTYPES;

pub(crate) fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    mr_ml::cosine_similarity(a, b)
}

pub(crate) fn l2_normalize(v: &[f64]) -> Vec<f64> {
    mr_ml::l2_normalize(v)
}

pub(crate) fn weighted_centroid(vecs: &[Vec<f64>], weights: &[f64]) -> Vec<f64> {
    mr_ml::weighted_centroid(vecs, weights)
}

/// Явный выброс выбираем только когда есть устойчивое ядро из ≥2 образцов:
/// иначе два равноправных прототипа нельзя рассудить без прослушивания.
const PROFILE_CORE_SIM: f64 = 0.55;
const PROFILE_OUTLIER_SIM: f64 = 0.45;
/// Ниже этого среднего профиль показывается как «образцы расходятся».
pub(crate) const PROFILE_MIXED_SIM: f64 = 0.50;

/// Средняя pairwise-согласованность прототипов и индексы явных выбросов.
/// Для двух прототипов выброс не выбирается: UI показывает только предупреждение.
pub(crate) fn prototype_consistency(embeddings: &[Vec<f64>]) -> (f64, Vec<usize>) {
    let n = embeddings.len();
    if n < 2 {
        return (1.0, vec![]);
    }
    let mut sums = vec![0.0f64; n];
    let mut total = 0.0f64;
    let mut pairs = 0usize;
    for i in 0..n {
        for j in (i + 1)..n {
            let similarity = cosine_similarity(&embeddings[i], &embeddings[j]);
            sums[i] += similarity;
            sums[j] += similarity;
            total += similarity;
            pairs += 1;
        }
    }
    let score = if pairs > 0 { total / pairs as f64 } else { 1.0 };
    if n < 3 {
        return (score, vec![]);
    }
    let means: Vec<f64> = sums.iter().map(|sum| sum / (n - 1) as f64).collect();
    let core = means
        .iter()
        .filter(|&&mean| mean >= PROFILE_CORE_SIM)
        .count();
    if core < 2 || core == n {
        return (score, vec![]);
    }
    let outliers = (0..n)
        .filter(|&index| means[index] < PROFILE_OUTLIER_SIM)
        .collect();
    (score, outliers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec<f64> {
        let norm = (x * x + y * y).sqrt();
        vec![x / norm, y / norm, 0.0, 0.0]
    }

    #[test]
    fn identical_prototypes_are_consistent() {
        let (score, outliers) = prototype_consistency(&[v(1.0, 0.0), v(1.0, 0.0)]);
        assert!((score - 1.0).abs() < 1e-9);
        assert!(outliers.is_empty());
    }

    #[test]
    fn two_different_prototypes_are_mixed_without_a_culprit() {
        let (score, outliers) = prototype_consistency(&[v(1.0, 0.0), v(0.0, 1.0)]);
        assert!(score < 0.1, "{score}");
        assert!(outliers.is_empty());
    }

    #[test]
    fn one_prototype_is_flagged_when_a_core_exists() {
        let (score, outliers) =
            prototype_consistency(&[v(1.0, 0.0), v(1.0, 0.0), v(1.0, 0.0), v(0.0, 1.0)]);
        assert!(score < 0.6, "{score}");
        assert_eq!(outliers, vec![3]);
    }
}
