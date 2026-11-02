//! Обёртка над словарём `mr-ml`, чтобы HTTP-слой не зависел от ML-крейта (P3).

pub(crate) const MAX_HOTWORDS: usize = mr_ml::hotwords::MAX_HOTWORDS;

pub(crate) fn sanitize(raw: &[String]) -> Vec<String> {
    mr_ml::hotwords::sanitize_hotwords(raw)
}
