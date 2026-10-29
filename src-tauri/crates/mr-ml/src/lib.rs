//! Математика диаризации: L2-нормализация, косинусное сходство, Венгерский алгоритм.
//!
//! * L2-нормализация, косинусное сходство
//! * Мульти-прототипный скоринг `S = 0.7·max + 0.3·mean_top2`
//! * Взвешенные центроиды `w = min(dur,8)·quality`
//! * Глобальное назначение через Венгерский алгоритм + Margin Gate
//! * ONNX-инференс — за cargo-фичей `onnx` (крейт `ort`), по умолчанию чистый CPU-математический слой.

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("пустой вектор")]
    EmptyVector,
    #[error("размерности не совпадают: {0} vs {1}")]
    DimMismatch(usize, usize),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, MlError>;

/// Чистая математика кластеризации (без ML-зависимостей): complete linkage
/// через nearest-neighbor chain + силуэт-оценка. Используется пайплайном диаризации.
pub mod cluster;

/// Идентификатор векторного пространства текущей embedding-модели
/// (`diar-eres2netv2-emb`, см. `models-manifest.json`).
pub const EMBEDDING_SPACE: &str = "eres2netv2-192-v1";
/// Размерность векторов пространства [`EMBEDDING_SPACE`].
pub const EMBEDDING_DIM: usize = 192;

/// Порог сходства «это тот же голос» для ERes2NetV2. Калибровка на официальных
/// примерах 3D-Speaker и реальных встречах: разные дикторы ≤ 0.22, один голос ≥ 0.48
/// (фрагменты от 1.5 с); у самой модели в карточке порог 0.365.
pub const DEFAULT_MATCH_THRESHOLD: f64 = 0.45;
/// Минимальный отрыв лучшего кандидата от второго (Margin Gate).
pub const DEFAULT_MIN_MARGIN: f64 = 0.06;
/// Сходство, при котором совпадение принимается без Margin Gate.
pub const HIGH_CONFIDENCE_THRESHOLD: f64 = 0.65;
/// Косинусное сходство кластеров для слияния раздробленных кластеров одного человека.
pub const SAME_SPEAKER_MERGE_SIM: f64 = 0.55;
/// Максимум прототипов одного спикера в voiceprints v2.
pub const MAX_PROTOTYPES: usize = 10;

/// Нормализация меток времени в канонический формат RFC3339 (P1, P9).
///
/// Ранние версии бэкенда сохраняли поле `updated_at` в `voiceprints.json v3`
/// как Unix epoch seconds в виде целых чисел (например, 1745778899) или числовых строк.
/// Десериализатор прозрачно нормализует числовые секунды в строку RFC3339 (например,
/// "2026-10-22T00:00:00Z"), а уже существующие строковые даты пропускает без изменений.
fn normalize_timestamp_rfc3339(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(secs) = trimmed.parse::<i64>() {
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
            return dt.to_rfc3339();
        }
    }
    trimmed.to_string()
}

fn deserialize_as_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrNumberVisitor;

    impl<'de> serde::de::Visitor<'de> for StringOrNumberVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an RFC3339 timestamp string or epoch number")
        }

        fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(normalize_timestamp_rfc3339(v))
        }

        fn visit_string<E>(self, v: String) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(normalize_timestamp_rfc3339(&v))
        }

        fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let s = chrono::DateTime::from_timestamp(v as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| v.to_string());
            Ok(s)
        }

        fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let s = chrono::DateTime::from_timestamp(v, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| v.to_string());
            Ok(s)
        }

        fn visit_f64<E>(self, v: f64) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let s = chrono::DateTime::from_timestamp(v as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| (v as u64).to_string());
            Ok(s)
        }
    }

    deserializer.deserialize_any(StringOrNumberVisitor)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prototype {
    pub id: String,
    pub embedding: Vec<f64>,
    #[serde(default = "default_dur")]
    pub duration_sec: f64,
    #[serde(default = "default_qual")]
    pub quality: f64,
    #[serde(default, deserialize_with = "deserialize_as_string")]
    pub created_at: String,
    pub snippet_relpath: Option<String>,
}

fn default_dur() -> f64 {
    4.0
}
fn default_qual() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerProfile {
    pub name: String,
    #[serde(default)]
    pub embedding: Vec<f64>,
    #[serde(default)]
    pub samples_count: u32,
    #[serde(default, deserialize_with = "deserialize_as_string")]
    pub updated_at: String,
    #[serde(default)]
    pub prototypes: Vec<Prototype>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchCandidate {
    pub name: String,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMatch {
    pub matched: bool,
    pub name: Option<String>,
    pub similarity: f64,
    pub margin: f64,
    #[serde(default)]
    pub candidates: Vec<MatchCandidate>,
}

/// L2-нормализация вектора (нулевой вектор возвращается как есть).
pub fn l2_normalize(v: &[f64]) -> Vec<f64> {
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm < 1e-12 {
        return vec![0.0; v.len()];
    }
    v.iter().map(|x| x / norm).collect()
}

/// Косинусное сходство двух векторов (0 при несовпадении размерностей).
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na < 1e-12 || nb < 1e-12 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Интервальный зазор между границами (0 при пересечении).
pub fn interval_gap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    (a_start - b_end).max(b_start - a_end).max(0.0)
}

/// Взвешенный центроид кластера: `w = min(dur, 8) · quality`, результат нормализован.
pub fn weighted_centroid(vecs: &[Vec<f64>], weights: &[f64]) -> Vec<f64> {
    if vecs.is_empty() {
        return vec![];
    }
    let dim = vecs[0].len();
    let mut acc = vec![0.0; dim];
    let mut total = 0.0;
    for (v, w) in vecs.iter().zip(weights.iter()) {
        if v.len() != dim {
            continue;
        }
        let ww = w.max(1e-6);
        for (i, val) in v.iter().enumerate() {
            acc[i] += val * ww;
        }
        total += ww;
    }
    if total <= 1e-12 {
        return vecs[0].clone();
    }
    let mean: Vec<f64> = acc.iter().map(|x| x / total).collect();
    l2_normalize(&mean)
}

/// Мульти-прототипный скоринг спикера.
pub fn score_speaker_similarity(query: &[f64], profile: &SpeakerProfile) -> f64 {
    let q = l2_normalize(query);
    if q.is_empty() {
        return 0.0;
    }
    let mut sims: Vec<f64> = profile
        .prototypes
        .iter()
        .filter(|p| p.embedding.len() == q.len())
        .map(|p| cosine_similarity(&q, &p.embedding))
        .collect();
    let c_sim = if profile.embedding.len() == q.len() {
        cosine_similarity(&q, &profile.embedding)
    } else {
        -1.0
    };
    if sims.is_empty() {
        return c_sim.max(0.0);
    }
    sims.sort_by(|a, b| b.total_cmp(a));
    let best = sims[0];
    if sims.len() == 1 {
        return best.max(c_sim);
    }
    let top2_mean = (sims[0] + sims[1]) / 2.0;
    (0.70 * best + 0.30 * top2_mean).max(c_sim)
}

/// Венгерский алгоритм (максимизация): возвращает пары (row, col).
pub fn linear_sum_assignment_max(score: &[Vec<f64>]) -> Vec<(usize, usize)> {
    let n_rows = score.len();
    if n_rows == 0 {
        return vec![];
    }
    let n_cols = score[0].len();
    if n_cols == 0 {
        return vec![];
    }
    // Транспонируем, если строк больше, чем столбцов
    let (mat, n, m, transposed): (Vec<Vec<f64>>, usize, usize, bool) = if n_rows > n_cols {
        let mut t = vec![vec![0.0; n_rows]; n_cols];
        for r in 0..n_rows {
            for c in 0..n_cols {
                t[c][r] = score[r][c];
            }
        }
        (t, n_cols, n_rows, true)
    } else {
        (score.to_vec(), n_rows, n_cols, false)
    };
    let max_val = mat
        .iter()
        .flat_map(|r| r.iter())
        .map(|x| x.max(0.0))
        .fold(0.0_f64, f64::max)
        + 1.0;
    let cost: Vec<Vec<f64>> = mat
        .iter()
        .map(|row| row.iter().map(|x| max_val - x.max(0.0)).collect())
        .collect();
    let mut u = vec![0.0; n + 1];
    let mut v = vec![0.0; m + 1];
    let mut p = vec![0usize; m + 1];
    let mut way = vec![0usize; m + 1];
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![f64::INFINITY; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = f64::INFINITY;
            let mut j1 = 0usize;
            for j in 1..=m {
                if !used[j] {
                    let cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut pairs = Vec::new();
    for (j, &pj) in p.iter().enumerate().skip(1) {
        if pj > 0 {
            let (r, c) = (pj - 1, j - 1);
            if transposed {
                pairs.push((c, r));
            } else {
                pairs.push((r, c));
            }
        }
    }
    pairs
}

/// Глобальное сопоставление кластеров с профилями (Венгерский + Margin Gate).
pub fn match_voiceprints_matrix(
    cluster_embeddings: &[Vec<f64>],
    profiles: &[SpeakerProfile],
    threshold: f64,
    min_margin: f64,
) -> Vec<ClusterMatch> {
    let n = cluster_embeddings.len();
    let mut results: Vec<ClusterMatch> = (0..n)
        .map(|_| ClusterMatch {
            matched: false,
            name: None,
            similarity: 0.0,
            margin: 0.0,
            candidates: vec![],
        })
        .collect();
    if n == 0 || profiles.is_empty() {
        return results;
    }
    let sim: Vec<Vec<f64>> = cluster_embeddings
        .iter()
        .map(|c| {
            profiles
                .iter()
                .map(|p| score_speaker_similarity(c, p))
                .collect()
        })
        .collect();
    for (i, row) in sim.iter().enumerate() {
        let mut ranked: Vec<(usize, f64)> = row.iter().cloned().enumerate().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let best = ranked.first().map(|x| x.1).unwrap_or(0.0);
        let second = ranked.get(1).map(|x| x.1).unwrap_or(0.0);
        results[i].similarity = (best.max(0.0) * 1000.0).round() / 1000.0;
        results[i].margin = ((best - second).max(0.0) * 1000.0).round() / 1000.0;
        results[i].candidates = ranked
            .iter()
            .take(3)
            .filter(|(_, s)| *s >= threshold * 0.85)
            .map(|(idx, s)| MatchCandidate {
                name: profiles[*idx].name.clone(),
                similarity: (s * 1000.0).round() / 1000.0,
            })
            .collect();
    }
    for (r, c) in linear_sum_assignment_max(&sim) {
        let s = sim[r][c];
        if s < threshold {
            continue;
        }
        let margin = results[r].margin;
        if profiles.len() > 1 && margin < min_margin && s < HIGH_CONFIDENCE_THRESHOLD {
            continue;
        }
        results[r].matched = true;
        results[r].name = Some(profiles[c].name.clone());
        results[r].similarity = (s * 1000.0).round() / 1000.0;
    }
    // Слияние раздробленных кластеров одного человека
    let assigned: std::collections::HashMap<usize, usize> = results
        .iter()
        .enumerate()
        .filter(|(_, x)| x.matched)
        .filter_map(|(ri, x)| {
            profiles
                .iter()
                .position(|p| Some(&p.name) == x.name.as_ref())
                .map(|pi| (pi, ri))
        })
        .collect();
    for i in 0..n {
        if results[i].matched {
            continue;
        }
        let mut best_pi = None;
        let mut best_s = 0.0;
        for (pi, _s) in sim[i].iter().enumerate() {
            if *_s > best_s {
                best_s = *_s;
                best_pi = Some(pi);
            }
        }
        if let Some(pi) = best_pi {
            if best_s >= threshold {
                if let Some(owner) = assigned.get(&pi) {
                    let inter =
                        cosine_similarity(&cluster_embeddings[i], &cluster_embeddings[*owner]);
                    if inter >= SAME_SPEAKER_MERGE_SIM {
                        results[i].matched = true;
                        results[i].name = Some(profiles[pi].name.clone());
                        results[i].similarity = (best_s * 1000.0).round() / 1000.0;
                    }
                }
            }
        }
    }
    results
}

#[cfg(feature = "onnx")]
pub mod onnx;

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(dim: usize, idx: usize) -> Vec<f64> {
        let mut v = vec![0.0; dim];
        v[idx % dim] = 1.0;
        v
    }
    fn l2(v: Vec<f64>) -> Vec<f64> {
        l2_normalize(&v)
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-9);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-9);
    }

    #[test]
    fn hungarian_prevents_collision() {
        let ivan = SpeakerProfile {
            name: "Иван".to_string(),
            embedding: unit(8, 0),
            samples_count: 1,
            updated_at: String::new(),
            prototypes: vec![Prototype {
                id: "p1".to_string(),
                embedding: unit(8, 0),
                duration_sec: 4.0,
                quality: 1.0,
                created_at: String::new(),
                snippet_relpath: None,
            }],
        };
        // Оба кластера близки к профилю (косинус 0.8), но межкластерный косинус
        // 0.28 < SAME_SPEAKER_MERGE_SIM, поэтому слияние фрагментов не срабатывает
        // и один профиль должен достаться только одному кластеру.
        let c1 = l2(vec![0.8, 0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let c2 = l2(vec![0.8, -0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let res = match_voiceprints_matrix(&[c1, c2], &[ivan], 0.45, 0.06);
        assert_eq!(res.len(), 2);
        assert!(res.iter().filter(|r| r.matched).count() <= 1);
    }

    #[test]
    fn margin_gate_ambiguous() {
        let mk = |name: &str, v: Vec<f64>| SpeakerProfile {
            name: name.to_string(),
            embedding: v.clone(),
            samples_count: 1,
            updated_at: String::new(),
            prototypes: vec![Prototype {
                id: "p1".to_string(),
                embedding: v,
                duration_sec: 4.0,
                quality: 1.0,
                created_at: String::new(),
                snippet_relpath: None,
            }],
        };
        let ivan = mk("Иван", l2(vec![1.0, 0.0, 0.0, 0.0]));
        let petr = mk("Петр", l2(vec![0.98, 0.2, 0.0, 0.0]));
        let q = l2(vec![0.99, 0.1, 0.0, 0.0]);
        let res = match_voiceprints_matrix(&[q], &[ivan, petr], 0.60, 0.06);
        assert_eq!(res.len(), 1);
        if !res[0].matched {
            assert!(res[0].candidates.len() >= 2);
        } else {
            assert!(res[0].similarity >= 0.78);
        }
    }

    #[test]
    fn weighted_centroid_ignores_noise() {
        let clean = l2(vec![1.0, 0.0, 0.0, 0.0]);
        let n1 = l2(vec![0.7, 0.7, 0.0, 0.0]);
        let n2 = l2(vec![0.7, -0.7, 0.0, 0.0]);
        let c = weighted_centroid(&[clean.clone(), n1, n2], &[8.0, 0.5, 0.5]);
        assert!(cosine_similarity(&c, &clean) > 0.996);
    }

    #[test]
    fn interval_gap_math() {
        assert!((interval_gap(5.8, 6.0, 6.1, 10.0) - 0.1).abs() < 1e-9);
        assert!(interval_gap(5.8, 6.0, 0.0, 2.0) > interval_gap(5.8, 6.0, 6.1, 10.0));
    }

    #[test]
    fn hungarian_finds_optimum() {
        // Максимум суммы: очевидный оптимум — (0,0)+(1,1) = 2.0.
        let score = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let mut pairs = linear_sum_assignment_max(&score);
        pairs.sort();
        assert_eq!(pairs, vec![(0, 0), (1, 1)]);
        // Прямоугольная матрица: строк больше столбцов — транспонирование внутри.
        let score = vec![vec![0.1], vec![0.9]];
        let pairs = linear_sum_assignment_max(&score);
        assert_eq!(pairs, vec![(1, 0)]);
        // Пустые входы.
        assert!(linear_sum_assignment_max(&[]) == vec![]);
        assert!(linear_sum_assignment_max(&[vec![]]) == vec![]);
    }

    #[test]
    fn test_deserialize_string_or_number() {
        let json_int = r#"{"name":"Тест","embedding":[0.1],"samples_count":1,"updated_at":1745778899,"prototypes":[{"id":"p1","name":"Тест","embedding":[0.1],"duration_sec":2.0,"quality":1.0,"created_at":1745778800,"snippet_relpath":null}]}"#;
        let profile: SpeakerProfile = serde_json::from_str(json_int).unwrap();
        assert!(profile.updated_at.starts_with("2025-04-27T18:34:59"));
        assert!(profile.prototypes[0]
            .created_at
            .starts_with("2025-04-27T18:33:20"));

        let json_str = r#"{"name":"Тест","embedding":[0.1],"samples_count":1,"updated_at":"2026-10-28T16:00:00Z","prototypes":[]}"#;
        let profile_str: SpeakerProfile = serde_json::from_str(json_str).unwrap();
        assert_eq!(profile_str.updated_at, "2026-10-28T16:00:00Z");
    }
}
