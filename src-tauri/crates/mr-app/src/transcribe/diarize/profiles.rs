use std::collections::HashMap;

use mr_ml::{ClusterMatch, SpeakerProfile};
use mr_packs as packs;

use crate::transcribe::{Sample, Utterance};

use super::MIN_NAMED_CLEAN_SEC;

pub(crate) fn pick_samples(utterances: &[Utterance], max_samples: usize) -> Vec<Sample> {
    let mut scored: Vec<(f64, &Utterance)> = utterances
        .iter()
        .map(|u| {
            let dur = (u.end - u.start).max(0.0);
            let words = u.text.split_whitespace().count() as f64;
            let dur_score = 1.0 - ((dur - 3.8).abs() / 3.8).min(1.0);
            (
                dur_score * 2.0 + (words / 10.0).min(1.0) + u.silhouette.clamp(-0.5, 1.0) * 1.5,
                u,
            )
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(max_samples)
        .map(|(_, u)| {
            let dur = (u.end - u.start).max(0.5);
            let start = (u.start * 100.0).round() / 100.0;
            // floor, а не round: окно сэмпла не заходит за конец реплики —
            // иначе в эталон голоса попадает до 50 мс речи следующего спикера.
            let sample_dur = ((dur.min(6.0) * 10.0).floor() / 10.0).max(0.1);
            let text = u.text.trim();
            let preview = if text.chars().count() > 98 {
                format!("{}…", text.chars().take(95).collect::<String>())
            } else {
                text.to_string()
            };
            Sample {
                start,
                end: ((start + sample_dur) * 100.0).round() / 100.0,
                dur: sample_dur,
                text: preview,
                full_text: text.to_string(),
                channel: u.channel,
            }
        })
        .collect()
}

/// Прочитать один `voiceprints.json`, отбросив чужое векторное пространство
/// и битые размерности. Профили без центроида, но с прототипами, собираются
/// из прототипов: иначе старый/импортированный пакет молча выпадает из матчинга.
pub(crate) fn read_profiles_file(file: &std::path::Path) -> Vec<SpeakerProfile> {
    let path = crate::logging::file_token(file);
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(error) => {
            if file.exists() {
                log::warn!(
                    target: "diar",
                    file = path,
                    reason = "read",
                    error = error.to_string();
                    "ev=diar.profile.load"
                );
            }
            return vec![];
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(error) => {
            log::warn!(
                target: "diar",
                file = path,
                reason = "json",
                error = error.to_string();
                "ev=diar.profile.load"
            );
            return vec![];
        }
    };
    let space = v
        .get("embedding_space")
        .and_then(|x| x.as_str())
        .unwrap_or(mr_ml::EMBEDDING_SPACE);
    if space != mr_ml::EMBEDDING_SPACE {
        log::warn!(
            target: "diar",
            file = path,
            reason = "embedding_space",
            space = space;
            "ev=diar.profile.load"
        );
        return vec![];
    }
    let arr = if let Some(a) = v.get("profiles").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.as_array() {
        a.clone()
    } else {
        log::warn!(
            target: "diar",
            file = path,
            reason = "no_profiles";
            "ev=diar.profile.load"
        );
        return vec![];
    };
    let total = arr.len();
    let mut dropped = 0usize;
    let mut out = vec![];
    for p in arr {
        let Ok(mut prof) = serde_json::from_value::<SpeakerProfile>(p) else {
            dropped += 1;
            continue;
        };
        if prof.name.trim().is_empty() {
            dropped += 1;
            continue;
        }
        prof.prototypes
            .retain(|x| x.embedding.len() == mr_ml::EMBEDDING_DIM);
        // Контракт v3: центроид собирается по актуальным прототипам.
        if !prof.prototypes.is_empty() {
            let vecs: Vec<Vec<f64>> = prof
                .prototypes
                .iter()
                .map(|x| x.embedding.clone())
                .collect();
            let ws: Vec<f64> = prof
                .prototypes
                .iter()
                .map(|x| x.duration_sec.max(0.5) * x.quality.max(0.1))
                .collect();
            prof.embedding = mr_ml::weighted_centroid(&vecs, &ws);
        }
        if prof.embedding.len() == mr_ml::EMBEDDING_DIM
            && prof.embedding.iter().any(|x| x.abs() > 1e-12)
        {
            out.push(prof);
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        log::warn!(
            target: "diar",
            file = path,
            reason = "invalid_profiles",
            kept = out.len(),
            dropped = dropped,
            total = total;
            "ev=diar.profile.load"
        );
    }
    out
}

/// Профили спикеров из включённых Voice Packs (v2/v3 + legacy-массив).
/// Корневой `voiceprints.json` читается первым как fallback: активные паки
/// важнее старых локальных данных.
pub(crate) fn load_profiles() -> Vec<SpeakerProfile> {
    let app = packs::default_app_data_dir();
    let mut files: Vec<(std::path::PathBuf, String, String)> = vec![];
    let root_vp = app.join("voiceprints.json");
    if root_vp.is_file() {
        files.push((
            root_vp,
            "default".to_string(),
            "Основной профиль".to_string(),
        ));
    }
    match packs::get_active_packs(&app) {
        Ok(active) => {
            for dir in active {
                let pid = dir
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let pname = match packs::get_pack_manifest(&app, &pid) {
                    Ok(manifest) => manifest.name,
                    Err(error) => {
                        log::warn!(
                            target: "diar",
                            file = pid.clone(),
                            reason = "pack_manifest",
                            error = error.to_string();
                            "ev=diar.profile.load"
                        );
                        pid.clone()
                    }
                };
                files.push((dir.join("voiceprints.json"), pid, pname));
            }
        }
        Err(error) => log::warn!(
            target: "diar",
            reason = "active_packs",
            error = error.to_string();
            "ev=diar.profile.load"
        ),
    }
    let mut by_name: HashMap<String, SpeakerProfile> = HashMap::new();
    for (f, pid, pname) in files {
        for mut prof in read_profiles_file(&f) {
            prof.pack_id = pid.clone();
            prof.pack_name = pname.clone();
            by_name.insert(prof.name.trim().to_lowercase(), prof);
        }
    }
    let mut out: Vec<SpeakerProfile> = by_name.into_values().collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Калибровка на коротких репликах, перепроверить на размеченном наборе.
const SHORT_REPLY_SEC: f64 = 1.2;
const SHORT_REPLY_WORDS: usize = 2;
const SHORT_REPLY_KEEP_SIM: f64 = 0.55;
const SHORT_REPLY_KEEP_MARGIN: f64 = 0.08;
const SHORT_REPLY_NEIGHBOR_GAP_SEC: f64 = 3.5;
const SHORT_REPLY_NEIGHBOR_SLACK: f64 = 0.02;

pub(super) fn empty_match() -> ClusterMatch {
    ClusterMatch {
        matched: false,
        name: None,
        pack_id: None,
        pack_name: None,
        similarity: 0.0,
        margin: 0.0,
        candidates: vec![],
    }
}

/// Индексы в `raw_clusters`, которым можно назначать имя профиля: только
/// кластеры с достаточным объёмом чистой (локальной) речи.
pub(super) fn eligible_clusters(
    raw_clusters: &[usize],
    label_of: &HashMap<usize, usize>,
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
) -> Vec<usize> {
    let mut clean_sec: HashMap<usize, f64> = HashMap::new();
    for (&index, &label) in label_of.iter() {
        if embs.get(index).is_some_and(Option::is_some) {
            *clean_sec.entry(label).or_default() += durs.get(index).copied().unwrap_or(0.0);
        }
    }
    (0..raw_clusters.len())
        .filter(|&k| clean_sec.get(&raw_clusters[k]).copied().unwrap_or(0.0) >= MIN_NAMED_CLEAN_SEC)
        .collect()
}

fn label_centroids(
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
    quals: &[f64],
    label_of: &HashMap<usize, usize>,
) -> HashMap<usize, Vec<f64>> {
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&index, &label) in label_of.iter() {
        if embs.get(index).is_some_and(Option::is_some) {
            groups.entry(label).or_default().push(index);
        }
    }
    groups
        .into_iter()
        .map(|(label, mut members)| {
            members.sort_unstable();
            let (vecs, ws): (Vec<Vec<f64>>, Vec<f64>) = members
                .iter()
                .filter_map(|&i| {
                    embs[i]
                        .clone()
                        .map(|vector| (vector, durs[i].min(8.0) * quals[i]))
                })
                .unzip();
            (label, mr_ml::weighted_centroid(&vecs, &ws))
        })
        .collect()
}

/// Короткая реплика («да», «угу») даёт неустойчивый вектор. Если акустика не
/// подтверждает её кластер, а обе соседние реплики в том же канале — один
/// голос, относим короткую к этому голосу: имена здесь ещё не назначены,
/// поэтому решение чисто акустическое и временное.
pub(super) fn reassign_short_replies(
    utterances: &[Utterance],
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
    quals: &[f64],
    label_of: &mut HashMap<usize, usize>,
) {
    let centroids = label_centroids(embs, durs, quals, label_of);
    if centroids.len() < 2 {
        return;
    }
    let mut moved = 0usize;
    for index in 0..utterances.len() {
        let Some(&current) = label_of.get(&index) else {
            continue;
        };
        let short = utterances[index].end - utterances[index].start < SHORT_REPLY_SEC
            || utterances[index].text.split_whitespace().count() <= SHORT_REPLY_WORDS;
        if !short {
            continue;
        }
        let Some(emb) = embs.get(index).and_then(Option::as_ref) else {
            continue;
        };
        let sim_current = centroids
            .get(&current)
            .map_or(0.0, |c| mr_ml::cosine_similarity(emb, c));
        let best_alt = centroids
            .iter()
            .filter(|(label, _)| **label != current)
            .map(|(_, c)| mr_ml::cosine_similarity(emb, c))
            .fold(f64::NEG_INFINITY, f64::max);
        if sim_current >= SHORT_REPLY_KEEP_SIM && sim_current - best_alt >= SHORT_REPLY_KEEP_MARGIN
        {
            continue;
        }
        let neighbor = |next: bool| -> Option<usize> {
            let utterance = &utterances[index];
            let mut best: Option<(f64, usize)> = None;
            for (other, candidate) in utterances.iter().enumerate() {
                if other == index
                    || candidate.channel != utterance.channel
                    || !label_of.contains_key(&other)
                {
                    continue;
                }
                let gap = if next {
                    if candidate.start < utterance.end {
                        continue;
                    }
                    candidate.start - utterance.end
                } else {
                    if candidate.end > utterance.start {
                        continue;
                    }
                    utterance.start - candidate.end
                };
                if gap > SHORT_REPLY_NEIGHBOR_GAP_SEC {
                    continue;
                }
                if best.is_none_or(|(best_gap, _)| gap < best_gap) {
                    best = Some((gap, other));
                }
            }
            best.map(|(_, other)| other)
        };
        let (Some(prev), Some(next)) = (neighbor(false), neighbor(true)) else {
            continue;
        };
        let (Some(&prev_label), Some(&next_label)) = (label_of.get(&prev), label_of.get(&next))
        else {
            continue;
        };
        if prev_label != next_label || prev_label == current {
            continue;
        }
        let sim_neighbor = centroids
            .get(&prev_label)
            .map_or(0.0, |c| mr_ml::cosine_similarity(emb, c));
        if sim_neighbor >= sim_current - SHORT_REPLY_NEIGHBOR_SLACK {
            label_of.insert(index, prev_label);
            moved += 1;
        }
    }
    if moved > 0 {
        log::debug!(
            target: "diar",
            moved = moved;
            "ev=diar.short_reply"
        );
    }
}
