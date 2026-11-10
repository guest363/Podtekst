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

pub mod cluster;
pub mod hotwords;

pub const EMBEDDING_SPACE: &str = "eres2netv2-192-v1";
pub const EMBEDDING_DIM: usize = 192;

/// Порог сходства «это тот же голос» для ERes2NetV2. Калибровка на официальных
/// примерах 3D-Speaker и реальных встречах: разные дикторы ≤ 0.22, один голос ≥ 0.48
/// (фрагменты от 1.5 с); у самой модели в карточке порог 0.365.
pub const DEFAULT_MATCH_THRESHOLD: f64 = 0.45;
pub const DEFAULT_MIN_MARGIN: f64 = 0.03;
// shortcut: 1.8/0.04 — стартовые значения без размеченного набора; перекалибровать на реальных ложных срабатываниях.
const MIN_COHORT_Z: f64 = 1.8;
const MIN_COHORT_SIGMA: f64 = 0.04;
const COHORT_TOP_K: usize = 5;
/// shortcut: 0.03 — эмпирический пол без опубликованного замера; перепроверить
/// на размеченном наборе при смене модели.
const MIN_MATCH_MARGIN_FLOOR: f64 = 0.03;
pub const HIGH_CONFIDENCE_THRESHOLD: f64 = 0.78;
pub const SAME_SPEAKER_MERGE_SIM: f64 = 0.55;
pub const MAX_PROTOTYPES: usize = 10;

/// Нормализация меток времени в канонический формат RFC3339 (P1, P9).
///
/// Десериализатор `updated_at`: принимает числовые epoch-секунды (нормализует в
/// строку RFC3339) и готовые строки RFC3339 (пропускает как есть).
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
    pub pack_id: String,
    #[serde(default)]
    pub pack_name: String,
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
    #[serde(default)]
    pub pack_id: String,
    #[serde(default)]
    pub pack_name: String,
    pub similarity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMatch {
    pub matched: bool,
    pub name: Option<String>,
    #[serde(default)]
    pub pack_id: Option<String>,
    #[serde(default)]
    pub pack_name: Option<String>,
    pub similarity: f64,
    pub margin: f64,
    #[serde(default)]
    pub candidates: Vec<MatchCandidate>,
}

pub fn l2_normalize(v: &[f64]) -> Vec<f64> {
    let norm = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm < 1e-12 {
        return vec![0.0; v.len()];
    }
    v.iter().map(|x| x / norm).collect()
}

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

pub fn interval_gap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    (a_start - b_end).max(b_start - a_end).max(0.0)
}

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

pub fn score_speaker_similarity(query: &[f64], profile: &SpeakerProfile) -> f64 {
    let q = l2_normalize(query);
    if q.is_empty() {
        return 0.0;
    }
    let mut sims: Vec<(f64, f64)> = profile
        .prototypes
        .iter()
        .filter(|p| p.embedding.len() == q.len())
        .map(|p| {
            let quality = (p.duration_sec.min(6.0) * p.quality).max(0.1);
            (cosine_similarity(&q, &p.embedding), quality)
        })
        .collect();
    let c_sim = if profile.embedding.len() == q.len() {
        cosine_similarity(&q, &profile.embedding)
    } else {
        -1.0
    };
    if sims.is_empty() {
        return c_sim.max(0.0);
    }
    sims.sort_by(|a, b| b.0.total_cmp(&a.0));
    if sims.len() == 1 {
        return sims[0].0.max(c_sim);
    }
    const NOMINAL: [f64; 3] = [0.55, 0.30, 0.15];
    let mut num = 0.0;
    let mut den = 0.0;
    for (i, &(similarity, quality)) in sims.iter().take(3).enumerate() {
        let weight = NOMINAL[i] * quality;
        num += weight * similarity;
        den += weight;
    }
    let weighted = if den > 0.0 { num / den } else { sims[0].0 };
    weighted.max(c_sim)
}

pub fn linear_sum_assignment_max(score: &[Vec<f64>]) -> Vec<(usize, usize)> {
    let n_rows = score.len();
    if n_rows == 0 {
        return vec![];
    }
    let n_cols = score[0].len();
    if n_cols == 0 {
        return vec![];
    }
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

fn cohort_z_score(row: &[f64], candidate: usize, score: f64) -> f64 {
    let mut cohort: Vec<f64> = row
        .iter()
        .enumerate()
        .filter(|&(index, _)| index != candidate)
        .map(|(_, &value)| value)
        .collect();
    if cohort.len() < 2 {
        return f64::INFINITY;
    }
    cohort.sort_by(|a, b| b.total_cmp(a));
    cohort.truncate(COHORT_TOP_K);
    let mean = cohort.iter().sum::<f64>() / cohort.len() as f64;
    let variance = cohort
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / cohort.len() as f64;
    (score - mean) / variance.sqrt().max(MIN_COHORT_SIGMA)
}

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
            pack_id: None,
            pack_name: None,
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
                pack_id: profiles[*idx].pack_id.clone(),
                pack_name: profiles[*idx].pack_name.clone(),
                similarity: (s * 1000.0).round() / 1000.0,
            })
            .collect();
    }
    for (r, c) in linear_sum_assignment_max(&sim) {
        let s = sim[r][c];
        if s < threshold {
            continue;
        }
        let other_max = sim[r]
            .iter()
            .enumerate()
            .filter(|&(idx, _)| idx != c)
            .map(|(_, &val)| val)
            .fold(f64::NEG_INFINITY, f64::max);
        let margin = s - other_max;
        if profiles.len() > 1
            && (margin < MIN_MATCH_MARGIN_FLOOR
                || (margin < min_margin && s < HIGH_CONFIDENCE_THRESHOLD))
        {
            continue;
        }
        if profiles.len() >= 4 && cohort_z_score(&sim[r], c, s) < MIN_COHORT_Z {
            continue;
        }
        results[r].matched = true;
        results[r].name = Some(profiles[c].name.clone());
        results[r].pack_id = if profiles[c].pack_id.is_empty() {
            None
        } else {
            Some(profiles[c].pack_id.clone())
        };
        results[r].pack_name = if profiles[c].pack_name.is_empty() {
            None
        } else {
            Some(profiles[c].pack_name.clone())
        };
        results[r].similarity = (s * 1000.0).round() / 1000.0;
        results[r].margin = (margin.max(0.0) * 1000.0).round() / 1000.0;
    }
    let mut assigned: std::collections::HashMap<usize, usize> = results
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
    let mut candidates: Vec<(usize, usize, f64, f64)> = Vec::new();
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
                let other_max = sim[i]
                    .iter()
                    .enumerate()
                    .filter(|&(idx, _)| idx != pi)
                    .map(|(_, &val)| val)
                    .fold(f64::NEG_INFINITY, f64::max);
                let margin = best_s - other_max;
                if profiles.len() > 1
                    && (margin < MIN_MATCH_MARGIN_FLOOR
                        || (margin < min_margin && best_s < HIGH_CONFIDENCE_THRESHOLD))
                {
                    continue;
                }
                if profiles.len() >= 4 && cohort_z_score(&sim[i], pi, best_s) < MIN_COHORT_Z {
                    continue;
                }
                candidates.push((i, pi, best_s, margin));
            }
        }
    }
    candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
    for (i, pi, best_s, margin) in candidates {
        if results[i].matched {
            continue;
        }
        let can_assign = match assigned.get(&pi) {
            Some(&owner) => {
                let inter = cosine_similarity(&cluster_embeddings[i], &cluster_embeddings[owner]);
                inter >= SAME_SPEAKER_MERGE_SIM
            }
            None => true,
        };
        if can_assign {
            assigned.entry(pi).or_insert(i);
            results[i].matched = true;
            results[i].name = Some(profiles[pi].name.clone());
            results[i].pack_id = if profiles[pi].pack_id.is_empty() {
                None
            } else {
                Some(profiles[pi].pack_id.clone())
            };
            results[i].pack_name = if profiles[pi].pack_name.is_empty() {
                None
            } else {
                Some(profiles[pi].pack_name.clone())
            };
            results[i].similarity = (best_s * 1000.0).round() / 1000.0;
            results[i].margin = (margin.max(0.0) * 1000.0).round() / 1000.0;
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
            pack_id: String::new(),
            pack_name: String::new(),
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
    fn hungarian_never_assigns_inferior_candidate_over_better_alternative() {
        let mk = |name: &str, v: Vec<f64>| SpeakerProfile {
            name: name.to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
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
        let prof_a = mk("Профиль A", unit(8, 0));
        let prof_b = mk("Профиль B", unit(8, 1));
        let c1 = l2(vec![0.95, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let c2 = l2(vec![0.85, 0.47, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let res = match_voiceprints_matrix(&[c1, c2], &[prof_a, prof_b], 0.45, 0.06);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].name.as_deref(), Some("Профиль A"));
        assert_ne!(res[1].name.as_deref(), Some("Профиль B"));
    }

    #[test]
    fn unassigned_profile_claimed_by_clean_cluster() {
        let mk = |name: &str, v: Vec<f64>| SpeakerProfile {
            name: name.to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
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
        let p_a = mk("Диктор A", unit(8, 0));
        let p_b = mk("Диктор B", unit(8, 1));
        let c1 = unit(8, 0);
        let c2 = unit(8, 1);
        let res = match_voiceprints_matrix(&[c1, c2], &[p_a, p_b], 0.45, 0.03);
        assert!(res[0].matched);
        assert_eq!(res[0].name.as_deref(), Some("Диктор A"));
        assert!(res[1].matched);
        assert_eq!(res[1].name.as_deref(), Some("Диктор B"));
    }

    #[test]
    fn margin_gate_ambiguous() {
        let mk = |name: &str, v: Vec<f64>| SpeakerProfile {
            name: name.to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
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

    fn profile(name: &str, v: Vec<f64>) -> SpeakerProfile {
        SpeakerProfile {
            name: name.to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
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
        }
    }

    #[test]
    fn hub_cluster_is_rejected_by_cohort_z() {
        let axis = |first: f64, dim: usize| {
            let mut v = vec![0.0; 8];
            v[0] = first;
            v[dim] = (1.0 - first * first).sqrt();
            v
        };
        let profiles = vec![
            profile("A", axis(0.52, 1)),
            profile("B", axis(0.45, 2)),
            profile("C", axis(0.45, 3)),
            profile("D", axis(0.45, 4)),
        ];
        let mut q = vec![0.0; 8];
        q[0] = 1.0;
        let res = match_voiceprints_matrix(&[q], &profiles, 0.45, 0.06);
        assert!(!res[0].matched, "хаб-вектор не должен матчиться: {res:?}");
        assert!(res[0].candidates.len() >= 2);
    }

    #[test]
    fn distinct_voice_passes_cohort_z() {
        let axis = |first: f64, dim: usize| {
            let mut v = vec![0.0; 8];
            v[0] = first;
            v[dim] = (1.0 - first * first).sqrt();
            v
        };
        let profiles = vec![
            profile("A", axis(0.9, 1)),
            profile("B", axis(0.2, 2)),
            profile("C", axis(0.18, 3)),
            profile("D", axis(0.16, 4)),
        ];
        let mut q = vec![0.0; 8];
        q[0] = 1.0;
        let res = match_voiceprints_matrix(&[q], &profiles, 0.45, 0.06);
        assert_eq!(res[0].name.as_deref(), Some("A"));
    }

    #[test]
    fn weighted_top3_uses_prototype_quality() {
        let good = l2(vec![1.0, 0.0, 0.0, 0.0]);
        let noisy = l2(vec![0.6, 0.8, 0.0, 0.0]);
        let mut profile = profile("A", good.clone());
        profile.prototypes = vec![
            Prototype {
                id: "good".to_string(),
                embedding: good,
                duration_sec: 6.0,
                quality: 1.0,
                created_at: String::new(),
                snippet_relpath: None,
            },
            Prototype {
                id: "noisy".to_string(),
                embedding: noisy,
                duration_sec: 0.5,
                quality: 0.5,
                created_at: String::new(),
                snippet_relpath: None,
            },
        ];
        let query = l2(vec![1.0, 0.0, 0.0, 0.0]);
        let score = score_speaker_similarity(&query, &profile);
        assert!(
            score > 0.9,
            "качественный прототип должен доминировать: {score}"
        );
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
        let score = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let mut pairs = linear_sum_assignment_max(&score);
        pairs.sort();
        assert_eq!(pairs, vec![(0, 0), (1, 1)]);
        let score = vec![vec![0.1], vec![0.9]];
        let pairs = linear_sum_assignment_max(&score);
        assert_eq!(pairs, vec![(1, 0)]);
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
