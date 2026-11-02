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
