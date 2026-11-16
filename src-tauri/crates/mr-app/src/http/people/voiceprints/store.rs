use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use mr_packs as packs;
use serde_json::json;

use super::super::lists::{ensure_person_in_pack, resolve_target_pack, same_name};
use super::super::lock_voiceprints;
use super::snippets::{remove_snippet, snippet_for_new_prototype};
use crate::config::app_data_dir;
use crate::fsutil::write_atomic;
use crate::voiceprints::{
    cosine_similarity, weighted_centroid, EMBEDDING_DIM, EMBEDDING_SPACE, MAX_PROTOTYPES,
};

static PROTO_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Профили из voiceprints.json любой схемы: массив (legacy) или v2/v3 `{"profiles": []}`.
pub(crate) fn parse_profiles(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.get("profiles")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_else(|| v.as_array().cloned().unwrap_or_default())
}

pub(crate) fn vp_v3(profiles: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"version": 3, "embedding_space": EMBEDDING_SPACE, "profiles": profiles})
}

/// Профиль пригоден для текущего embedding-пространства: есть центроид или
/// хотя бы один прототип нужной размерности.
pub(super) fn voiceprint_usable(p: &serde_json::Value) -> bool {
    let emb_ok = p
        .get("embedding")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.len() == EMBEDDING_DIM && a.iter().filter_map(|x| x.as_f64()).any(|x| x.abs() > 1e-12)
        })
        .unwrap_or(false);
    let proto_ok = p
        .get("prototypes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter().any(|x| {
                x.get("embedding")
                    .and_then(|v| v.as_array())
                    .map(|e| {
                        e.len() == EMBEDDING_DIM
                            && e.iter().filter_map(|n| n.as_f64()).any(|n| n.abs() > 1e-12)
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    emb_ok || proto_ok
}

fn prototype_duration(start: Option<f64>, end: Option<f64>) -> f64 {
    match (start, end) {
        (Some(s), Some(e)) if e > s => (e - s).clamp(0.4, 15.0),
        _ => 4.0,
    }
}

pub(crate) fn prototypes_centroid(protos: &[serde_json::Value]) -> (Vec<f64>, usize) {
    let (vecs, weights): (Vec<Vec<f64>>, Vec<f64>) = protos
        .iter()
        .filter_map(|x| {
            let vec: Vec<f64> = x
                .get("embedding")?
                .as_array()?
                .iter()
                .filter_map(|n| n.as_f64())
                .collect();
            if vec.len() != EMBEDDING_DIM {
                return None;
            }
            let duration = x
                .get("duration_sec")
                .and_then(|v| v.as_f64())
                .unwrap_or(4.0)
                .max(0.1);
            let quality = x
                .get("quality")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0)
                .max(0.1);
            Some((vec, duration * quality))
        })
        .unzip();
    let count = vecs.len();
    let centroid = if count == 0 {
        vec![]
    } else {
        weighted_centroid(&vecs, &weights)
    };
    (centroid, count)
}

#[derive(Default)]
pub(crate) struct PrototypeSample<'a> {
    pub(crate) audio_file: &'a str,
    pub(crate) start: Option<f64>,
    pub(crate) end: Option<f64>,
    pub(crate) channel: Option<usize>,
    pub(crate) quality: Option<f64>,
}

/// Битый файл или чужое пространство — ошибка, а не пустой список: иначе
/// следующая запись молча стирает все накопленные отпечатки.
fn load_pack_profiles(path: &Path) -> Result<Vec<serde_json::Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Не удалось прочитать голосовые отпечатки: {error}")),
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        "Файл голосовых отпечатков повреждён: сохранение стёрло бы записанные голоса".to_string()
    })?;
    if value
        .get("embedding_space")
        .and_then(|x| x.as_str())
        .is_some_and(|space| space != EMBEDDING_SPACE)
    {
        return Err(
            "Голосовые отпечатки пакета собраны другой моделью: сохраните голос в новый пакет"
                .to_string(),
        );
    }
    Ok(parse_profiles(&value))
}

pub(crate) fn store_voiceprint(
    name: &str,
    target_pack_id: Option<&str>,
    norm: Vec<f64>,
    sample: PrototypeSample<'_>,
) -> Result<(), String> {
    let chosen_pack = resolve_target_pack(name, target_pack_id);
    let now = crate::recording::now_secs();
    let now_iso = chrono::Utc::now().to_rfc3339();
    let pack_dir = packs::packs_root_dir(&app_data_dir()).join(&chosen_pack);
    let _ = std::fs::create_dir_all(pack_dir.join("snippets"));
    let vp_file = pack_dir.join("voiceprints.json");
    if let Some(parent) = vp_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Сниппет собираем до lock: декодирование WAV не должно задерживать
    // параллельные сохранения голосов.
    let new_id = format!("p{now}-{}", PROTO_SEQ.fetch_add(1, Ordering::Relaxed) + 1);
    let snippet_rel = snippet_for_new_prototype(
        sample.audio_file,
        &new_id,
        sample.start,
        sample.end,
        sample.channel,
        vp_file.parent(),
    );

    let _guard = lock_voiceprints();
    let mut profiles = match load_pack_profiles(&vp_file) {
        Ok(profiles) => profiles,
        Err(error) => {
            drop(_guard);
            if let (Some(rel), Some(parent)) = (snippet_rel.as_deref(), vp_file.parent()) {
                remove_snippet(parent, rel);
            }
            return Err(error);
        }
    };
    profiles.retain(voiceprint_usable);
    let duration_sec = prototype_duration(sample.start, sample.end);
    let quality_val = sample.quality.unwrap_or(1.0).clamp(0.1, 2.0);
    let mut duplicate = false;
    if let Some(p) = profiles.iter_mut().find(|p| {
        p.get("name")
            .and_then(|x| x.as_str())
            .map(|n| same_name(n, name))
            .unwrap_or(false)
    }) {
        let mut protos: Vec<serde_json::Value> = p
            .get("prototypes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        protos.retain(|x| {
            x.get("embedding")
                .and_then(|v| v.as_array())
                .map(|a| a.len() == norm.len())
                .unwrap_or(false)
        });
        duplicate = protos.iter().any(|x| {
            x.get("embedding")
                .and_then(|v| v.as_array())
                .map(|a| {
                    let v: Vec<f64> = a.iter().filter_map(|n| n.as_f64()).collect();
                    cosine_similarity(&v, &norm) >= 0.98
                })
                .unwrap_or(false)
        });
        if !duplicate && protos.len() >= 3 {
            let best = protos
                .iter()
                .filter_map(|x| {
                    x.get("embedding")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|n| n.as_f64()).collect::<Vec<f64>>())
                })
                .map(|v| cosine_similarity(&v, &norm))
                .fold(-1.0f64, f64::max);
            if best < 0.30 {
                drop(_guard);
                if let (Some(rel), Some(parent)) = (snippet_rel.as_deref(), vp_file.parent()) {
                    remove_snippet(parent, rel);
                }
                return Err(
                    "Выбранный фрагмент звучит как другой голос или содержит сильный шум"
                        .to_string(),
                );
            }
        }
        if !duplicate {
            protos.push(json!({
                "id": new_id,
                "embedding": norm,
                "duration_sec": duration_sec,
                "quality": quality_val,
                "created_at": now_iso.clone(),
                "snippet_relpath": snippet_rel.clone(),
            }));
        }
        while protos.len() > MAX_PROTOTYPES {
            let worst_idx = protos
                .iter()
                .enumerate()
                .map(|(idx, p)| {
                    let q = p.get("quality").and_then(|v| v.as_f64()).unwrap_or(1.0);
                    let d = p
                        .get("duration_sec")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(4.0)
                        .min(6.0);
                    (idx, q * d)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            let removed = protos.remove(worst_idx);
            if let (Some(rel), Some(parent)) = (
                removed.get("snippet_relpath").and_then(|v| v.as_str()),
                vp_file.parent(),
            ) {
                remove_snippet(parent, rel);
            }
        }
        let (centroid, count) = prototypes_centroid(&protos);
        if count > 0 {
            p["embedding"] = json!(centroid);
            p["samples_count"] = json!(count as u64);
        }
        p["name"] = json!(name);
        p["prototypes"] = json!(protos);
        p["updated_at"] = json!(now_iso.clone());
    } else {
        profiles.push(json!({
            "name": name,
            "embedding": norm.clone(),
            "samples_count": 1,
            "updated_at": now_iso.clone(),
            "prototypes": [{
                "id": new_id,
                "embedding": norm,
                "duration_sec": duration_sec,
                "quality": quality_val,
                "created_at": now_iso,
                "snippet_relpath": snippet_rel.clone(),
            }],
        }));
    }
    let _ = write_atomic(
        &vp_file,
        serde_json::to_string_pretty(&vp_v3(profiles))
            .unwrap_or_default()
            .as_bytes(),
    );
    drop(_guard);
    // Дубликат не попал в профиль — сниппет ему не нужен.
    if duplicate {
        if let (Some(rel), Some(parent)) = (snippet_rel.as_deref(), vp_file.parent()) {
            remove_snippet(parent, rel);
        }
    }
    ensure_person_in_pack(name, Some("Участник"), &chosen_pack);
    Ok(())
}

pub(super) fn voiceprint_files() -> Vec<std::path::PathBuf> {
    let mut files = vec![
        app_data_dir().join("voiceprints.json"),
        app_data_dir().join("packs/default/voiceprints.json"),
    ];
    if let Ok(packs) = packs::get_active_packs(&app_data_dir()) {
        for dir in packs {
            let f = dir.join("voiceprints.json");
            if !files.contains(&f) {
                files.push(f);
            }
        }
    }
    files
}

pub(crate) fn pack_voiceprint_file(pack_id: &str) -> std::path::PathBuf {
    packs::packs_root_dir(&app_data_dir())
        .join(pack_id)
        .join("voiceprints.json")
}

pub(crate) fn delete_voiceprint_files(name: &str, files: Vec<std::path::PathBuf>) -> bool {
    let clean = name.trim();
    if clean.is_empty() {
        return false;
    }
    let mut found = false;
    for vp_file in files {
        let _guard = lock_voiceprints();
        let Ok(t) = std::fs::read_to_string(&vp_file) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
            continue;
        };
        let arr = parse_profiles(&v);
        let before = arr.len();
        let mut snippets_to_remove = Vec::new();
        let kept: Vec<_> = arr
            .into_iter()
            .filter(|p| {
                let hit = same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), clean);
                if hit {
                    if let Some(protos) = p.get("prototypes").and_then(|x| x.as_array()) {
                        for pr in protos {
                            if let Some(sn) = pr.get("snippet_relpath").and_then(|x| x.as_str()) {
                                snippets_to_remove.push(sn.to_string());
                            }
                        }
                    }
                }
                !hit
            })
            .collect();
        if kept.len() != before {
            found = true;
            let _ = write_atomic(
                &vp_file,
                serde_json::to_string_pretty(&vp_v3(kept))
                    .unwrap_or_default()
                    .as_bytes(),
            );
            if let Some(parent) = vp_file.parent() {
                for sn in snippets_to_remove {
                    remove_snippet(parent, &sn);
                }
            }
        }
    }
    found
}

/// pack_id для URL сниппета: "" — корень app_data, иначе имя папки пакета.
pub(super) fn pack_id_of(vp_file: &Path) -> Option<String> {
    if vp_file == app_data_dir().join("voiceprints.json") {
        return Some(String::new());
    }
    let root = mr_packs::packs_root_dir(&app_data_dir());
    let rel = vp_file.strip_prefix(&root).ok()?;
    if rel.components().count() == 2 {
        rel.components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
    } else {
        None
    }
}
