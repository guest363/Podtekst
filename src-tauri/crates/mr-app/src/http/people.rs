use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use mr_packs as packs;
use serde_json::json;

use super::json_response;
use super::media::inside_save_dir;
use crate::config::{app_data_dir, save_dir};
use crate::fsutil::write_atomic;
use crate::voiceprints::{
    cosine_similarity, l2_normalize, weighted_centroid, EMBEDDING_DIM, EMBEDDING_SPACE,
    MAX_PROTOTYPES,
};

/// Сериализует read-modify-write `voiceprints.json`: иначе два параллельных
/// сохранения голоса теряют одно из изменений.
static VOICEPRINTS_LOCK: Mutex<()> = Mutex::new(());
static PROTO_SEQ: AtomicUsize = AtomicUsize::new(0);

fn lock_voiceprints() -> std::sync::MutexGuard<'static, ()> {
    VOICEPRINTS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn external_people_path() -> PathBuf {
    app_data_dir().join("external_people.json")
}

pub(super) fn load_external_people() -> Vec<serde_json::Value> {
    let path = external_people_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    target: "json",
                    path = crate::logging::file_token(&path),
                    error = error.to_string();
                    "ev=json.load"
                );
            }
            return Vec::new();
        }
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value.as_array().cloned().unwrap_or_default(),
        Err(error) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(&path),
                error = error.to_string();
                "ev=json.load"
            );
            Vec::new()
        }
    }
}

pub(super) fn save_external_people(list: &[serde_json::Value]) {
    let _ = write_atomic(
        &external_people_path(),
        serde_json::to_string_pretty(&json!(list))
            .unwrap_or_default()
            .as_bytes(),
    );
}

pub(super) fn people_json_files() -> Vec<PathBuf> {
    packs::list_packs(&app_data_dir())
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.path.join("people.json"))
        .collect()
}

/// Профили из voiceprints.json любой схемы: массив (legacy) или v2/v3 `{"profiles": []}`.
fn parse_profiles(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.get("profiles")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_else(|| v.as_array().cloned().unwrap_or_default())
}

fn vp_v3(profiles: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"version": 3, "embedding_space": EMBEDDING_SPACE, "profiles": profiles})
}

/// Контакт в people.json не затрагивается, пропадает только голосовой вектор.
pub(crate) fn migrate_voiceprints_v3() {
    let mut files = vec![app_data_dir().join("voiceprints.json")];
    for p in people_json_files() {
        if let Some(dir) = p.parent() {
            files.push(dir.join("voiceprints.json"));
        }
    }
    files.sort();
    files.dedup();
    for file in files {
        let _guard = lock_voiceprints();
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let v = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => value,
            Err(error) => {
                log::warn!(
                    target: "json",
                    path = crate::logging::file_token(&file),
                    error = error.to_string();
                    "ev=json.load"
                );
                continue;
            }
        };
        let profiles = parse_profiles(&v);
        let n_before = profiles.len();
        let space_ok = v
            .get("embedding_space")
            .and_then(|x| x.as_str())
            .map(|s| s == EMBEDDING_SPACE)
            .unwrap_or(true);
        let kept: Vec<_> = if space_ok {
            profiles
                .into_iter()
                .filter(|p| {
                    p.get("embedding")
                        .and_then(|x| x.as_array())
                        .map(|a| a.len() == EMBEDDING_DIM)
                        .unwrap_or(true)
                })
                .collect()
        } else {
            vec![]
        };
        if kept.len() == n_before {
            continue;
        }
        let _ = std::fs::copy(&file, file.with_extension("v2.bak"));
        let _ = write_atomic(
            &file,
            serde_json::to_string_pretty(&vp_v3(kept))
                .unwrap_or_default()
                .as_bytes(),
        );
    }
}

pub(super) fn read_people_list(path: &Path) -> Option<Vec<serde_json::Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    target: "json",
                    path = crate::logging::file_token(path),
                    error = error.to_string();
                    "ev=json.load"
                );
            }
            return None;
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(path),
                error = error.to_string();
                "ev=json.load"
            );
            return None;
        }
    };
    if let Some(arr) = v.as_array() {
        return Some(arr.clone());
    }
    v.as_object().map(|map| {
        map.iter()
            .map(|(k, r)| json!({"name": k, "role": r}))
            .collect()
    })
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Переименовывает профили голоса во всех базах, чтобы привязка не потерялась.
fn rename_voiceprint_names(old: &str, new: &str) -> bool {
    let mut files = vec![app_data_dir().join("voiceprints.json")];
    for p in people_json_files() {
        if let Some(dir) = p.parent() {
            files.push(dir.join("voiceprints.json"));
        }
    }
    files.sort();
    files.dedup();
    let mut found = false;
    for file in files {
        let _guard = lock_voiceprints();
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                let mut profiles = parse_profiles(&v);
                let mut changed = false;
                for p in &mut profiles {
                    if p.get("name")
                        .and_then(|x| x.as_str())
                        .map(|n| same_name(n, old))
                        .unwrap_or(false)
                    {
                        p["name"] = json!(new);
                        changed = true;
                        found = true;
                    }
                }
                if changed {
                    let _ = write_atomic(
                        &file,
                        serde_json::to_string_pretty(&vp_v3(profiles))
                            .unwrap_or_default()
                            .as_bytes(),
                    );
                }
            }
        }
    }
    found
}

pub(super) fn update_people(
    old: &str,
    new_name: &str,
    role: Option<&str>,
    target_pack_id: Option<&str>,
    from_pack_id: Option<&str>,
) -> bool {
    let mut moved = false;
    if let Some(target_id) = target_pack_id.filter(|s| !s.trim().is_empty()) {
        let from_pack = from_pack_id
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| resolve_target_pack(old, None));
        if from_pack != target_id {
            if let Err(e) = packs::move_person_pack(&app_data_dir(), old, &from_pack, target_id) {
                crate::note!(
                    "[warn] move_person_pack {old} from {from_pack} to {target_id}: {e:?}"
                );
            }
            // Гарантируем отсутствие дублей в остальных пакетах
            if let Ok(all_packs) = packs::list_packs(&app_data_dir()) {
                for p_info in all_packs {
                    if p_info.id != target_id {
                        let _ = packs::remove_person_from_pack(&app_data_dir(), &p_info.id, old);
                    }
                }
            }
            let mut ext = load_external_people();
            let before = ext.len();
            ext.retain(|p| !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), old));
            if ext.len() != before {
                save_external_people(&ext);
            }
            moved = true;
        }
    }
    let mut found = moved;
    for file in people_json_files() {
        let Some(mut list) = read_people_list(&file) else {
            continue;
        };
        let mut changed = false;
        for p in &mut list {
            let name = p.get("name").and_then(|x| x.as_str()).unwrap_or("");
            if same_name(name, old) {
                p["name"] = json!(new_name);
                if let Some(r) = role.filter(|r| !r.trim().is_empty()) {
                    p["role"] = json!(r);
                }
                changed = true;
                found = true;
            }
        }
        if changed {
            let _ = write_atomic(
                &file,
                serde_json::to_string_pretty(&list)
                    .unwrap_or_default()
                    .as_bytes(),
            );
        }
    }
    let mut ext = load_external_people();
    let mut changed = false;
    for p in &mut ext {
        let name = p.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if same_name(name, old) {
            p["name"] = json!(new_name);
            if let Some(r) = role.filter(|r| !r.trim().is_empty()) {
                p["role"] = json!(r);
            }
            changed = true;
            found = true;
        }
    }
    if changed {
        save_external_people(&ext);
    }
    if found && !same_name(old, new_name) {
        rename_voiceprint_names(old, new_name);
    }
    found
}

/// Убирает человека из пакетов, списка гостей и удаляет отпечатки голоса,
/// чтобы автосинхронизация не восстанавливала удалённого участника.
pub(super) fn remove_person(name: &str) -> bool {
    let mut found = false;
    for file in people_json_files() {
        let Some(list) = read_people_list(&file) else {
            continue;
        };
        let before = list.len();
        let kept: Vec<_> = list
            .into_iter()
            .filter(|p| !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
            .collect();
        if kept.len() != before {
            found = true;
            let _ = write_atomic(
                &file,
                serde_json::to_string_pretty(&kept)
                    .unwrap_or_default()
                    .as_bytes(),
            );
        }
    }
    let mut ext = load_external_people();
    let before = ext.len();
    ext.retain(|p| !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), name));
    if ext.len() != before {
        found = true;
        save_external_people(&ext);
    }
    let vp_found = delete_voiceprint_internal(name, None);
    found || vp_found
}

/// Профиль пригоден для текущего embedding-пространства: есть центроид или
/// хотя бы один прототип нужной размерности.
fn voiceprint_usable(p: &serde_json::Value) -> bool {
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

pub(super) fn load_all_voiceprints() -> Vec<serde_json::Value> {
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut push = |p: serde_json::Value| {
        let n = p
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if !n.is_empty() && voiceprint_usable(&p) && seen.insert(n) {
            out.push(p);
        }
    };
    if let Ok(packs) = packs::get_active_packs(&app_data_dir()) {
        for dir in packs {
            let pid = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("default")
                .to_string();
            let pname = packs::get_pack_manifest(&app_data_dir(), &pid)
                .map(|m| m.name)
                .unwrap_or_else(|_| "Основной профиль".to_string());
            if let Ok(t) = std::fs::read_to_string(dir.join("voiceprints.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("embedding_space")
                        .and_then(|x| x.as_str())
                        .is_some_and(|s| s != EMBEDDING_SPACE)
                    {
                        continue;
                    }
                    for mut p in parse_profiles(&v) {
                        p["pack_id"] = json!(pid);
                        p["pack_name"] = json!(pname);
                        push(p);
                    }
                }
            }
        }
    }
    let root_vp = app_data_dir().join("voiceprints.json");
    if root_vp.is_file() {
        if let Ok(t) = std::fs::read_to_string(&root_vp) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if v.get("embedding_space")
                    .and_then(|x| x.as_str())
                    .is_some_and(|s| s != EMBEDDING_SPACE)
                {
                    return out;
                }
                for p in parse_profiles(&v) {
                    push(p);
                }
            }
        }
    }
    out
}

pub(super) fn resolve_target_pack(name: &str, preferred_pack_id: Option<&str>) -> String {
    if let Some(pid) = preferred_pack_id {
        let clean = pid.trim();
        let path = Path::new(clean);
        let mut parts = path.components();
        let single = !path.is_absolute()
            && matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none();
        if single {
            let root = packs::packs_root_dir(&app_data_dir());
            if root.join(clean).is_dir() {
                return clean.to_string();
            }
        }
    }
    let clean_name = name.trim();
    if !clean_name.is_empty() {
        if let Ok(active) = packs::get_active_packs(&app_data_dir()) {
            for dir in active {
                let pf = dir.join("people.json");
                if let Some(list) = read_people_list(&pf) {
                    if list.iter().any(|p| {
                        same_name(
                            p.get("name").and_then(|x| x.as_str()).unwrap_or(""),
                            clean_name,
                        )
                    }) {
                        if let Some(n) = dir.file_name() {
                            return n.to_string_lossy().to_string();
                        }
                    }
                }
            }
        }
    }
    "default".to_string()
}

pub(super) fn ensure_person_in_pack(name: &str, role: Option<&str>, pack_id: &str) {
    let clean = name.trim();
    if clean.is_empty() {
        return;
    }
    let mut ext = load_external_people();
    let before_ext = ext.len();
    ext.retain(|p| !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), clean));
    if ext.len() != before_ext {
        save_external_people(&ext);
    }
    let pf = packs::packs_root_dir(&app_data_dir())
        .join(pack_id)
        .join("people.json");
    let in_pack = read_people_list(&pf).unwrap_or_default().iter().any(|p| {
        p.get("name")
            .and_then(|x| x.as_str())
            .map(|n| same_name(n, clean))
            .unwrap_or(false)
    });
    if !in_pack {
        let _ = packs::add_person_to_pack(
            &app_data_dir(),
            pack_id,
            json!({
                "name": clean,
                "role": role.unwrap_or("Участник")
            }),
        );
    }
}

pub(super) fn ensure_person_in_list(name: &str, role: Option<&str>) {
    let target = resolve_target_pack(name, None);
    ensure_person_in_pack(name, role, &target);
}

pub(crate) fn sync_voiceprint_people() {
    for vp in load_all_voiceprints() {
        if let Some(name) = vp.get("name").and_then(|x| x.as_str()) {
            ensure_person_in_list(name, Some("Участник"));
        }
    }
}

fn prototype_duration(start: Option<f64>, end: Option<f64>) -> f64 {
    match (start, end) {
        (Some(s), Some(e)) if e > s => (e - s).clamp(0.4, 15.0),
        _ => 4.0,
    }
}

fn prototypes_centroid(protos: &[serde_json::Value]) -> (Vec<f64>, usize) {
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
struct PrototypeSample<'a> {
    audio_file: &'a str,
    start: Option<f64>,
    end: Option<f64>,
    channel: Option<usize>,
    quality: Option<f64>,
}

fn store_voiceprint(
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
    let mut profiles: Vec<serde_json::Value> = match std::fs::read_to_string(&vp_file) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                if value
                    .get("embedding_space")
                    .and_then(|x| x.as_str())
                    .is_some_and(|s| s != EMBEDDING_SPACE)
                {
                    vec![]
                } else {
                    parse_profiles(&value)
                }
            }
            Err(error) => {
                log::warn!(
                    target: "json",
                    path = crate::logging::file_token(&vp_file),
                    error = error.to_string();
                    "ev=json.load"
                );
                vec![]
            }
        },
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    target: "json",
                    path = crate::logging::file_token(&vp_file),
                    error = error.to_string();
                    "ev=json.load"
                );
            }
            vec![]
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

pub(super) fn handle_voiceprint_save(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let pack_id = body
        .get("pack_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    // Без onnx эмбеддинг из аудио не извлекается, поэтому mut не нужен.
    #[cfg_attr(not(feature = "onnx"), allow(unused_mut))]
    let mut emb: Vec<f64> = body
        .get("embedding")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
        .unwrap_or_default();
    let audio_file = body
        .get("audio_file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let old_name = body
        .get("old_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Границы образца для сниппета «как звучит»: берутся из карточки спикера.
    let sample_start = body.get("sample_start").and_then(|v| v.as_f64());
    let sample_end = body.get("sample_end").and_then(|v| v.as_f64());
    let sample_channel = body
        .get("sample_channel")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let mut extracted_quality: Option<f64> = None;
    #[cfg(feature = "onnx")]
    if emb.is_empty() && !audio_file.is_empty() {
        if let (Some(start), Some(end)) = (sample_start, sample_end) {
            let audio_p = Path::new(&audio_file);
            let full_p = if audio_p.is_absolute() {
                audio_p.to_path_buf()
            } else {
                save_dir().join(audio_p)
            };
            if inside_save_dir(&full_p) {
                let channel_i32 = sample_channel.map(|c| c as i32);
                match crate::transcribe::extract_utterance_embedding(
                    &full_p,
                    start,
                    end,
                    channel_i32,
                ) {
                    Ok((e, _dur, q)) => {
                        emb = e;
                        extracted_quality = Some(q);
                    }
                    Err(error) => {
                        json_response(request, json!({"ok": false, "error": error}), 422);
                        return;
                    }
                }
            }
        }
    }
    if name.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Укажите имя участника"}),
            400,
        );
        return;
    }
    if !emb.is_empty() && emb.len() != EMBEDDING_DIM {
        json_response(
            request,
            json!({"ok": false, "error": "Неверная размерность голосового отпечатка"}),
            400,
        );
        return;
    }
    let chosen_pack = resolve_target_pack(&name, pack_id.as_deref());
    if !emb.is_empty() {
        let norm = l2_normalize(&emb);
        if norm.iter().all(|x| x.abs() < 1e-12) {
            json_response(
                request,
                json!({"ok": false, "error": "Пустой голосовой отпечаток"}),
                400,
            );
            return;
        }
        if let Err(error) = store_voiceprint(
            &name,
            Some(&chosen_pack),
            norm,
            PrototypeSample {
                audio_file: &audio_file,
                start: sample_start,
                end: sample_end,
                channel: sample_channel,
                quality: extracted_quality,
            },
        ) {
            json_response(request, json!({"ok": false, "error": error}), 422);
            return;
        }
    }

    ensure_person_in_pack(&name, Some("Участник"), &chosen_pack);
    if !audio_file.is_empty() {
        // Правим расшифровки только внутри папки записей (запись в произвольную
        // директорию через чужое имя файла недопустима).
        if inside_save_dir(Path::new(&audio_file)) {
            rename_speaker_in_files(
                Path::new(&audio_file),
                &name,
                if old_name.is_empty() {
                    None
                } else {
                    Some(old_name.as_str())
                },
            );
        }
    }
    json_response(
        request,
        json!({"ok": true, "voiceprint_saved": !emb.is_empty(), "pack_id": chosen_pack}),
        200,
    );
}

pub(super) fn handle_people_add(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let role = body
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("Участник")
        .trim()
        .to_string();
    let pack_id = body
        .get("pack_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .trim()
        .to_string();
    if name.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Укажите имя участника"}),
            400,
        );
        return;
    }
    match packs::add_person_to_pack(
        &app_data_dir(),
        &pack_id,
        json!({"name": name, "role": role}),
    ) {
        Ok(_) => json_response(request, json!({"ok": true}), 200),
        Err(e) => json_response(request, json!({"ok": false, "error": e.to_string()}), 500),
    }
}

pub(super) fn handle_people_move(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let to_pack = body
        .get("to_pack_id")
        .or_else(|| body.get("pack_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let from_pack = body
        .get("from_pack_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if name.is_empty() || to_pack.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Укажите имя участника и целевой голосовой пакет"}),
            400,
        );
        return;
    }
    let from_pack = if from_pack.is_empty() {
        resolve_target_pack(&name, None)
    } else {
        from_pack
    };
    match packs::move_person_pack(&app_data_dir(), &name, &from_pack, &to_pack) {
        Ok(_) => {
            let mut ext = load_external_people();
            let before = ext.len();
            ext.retain(|p| !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), &name));
            if ext.len() != before {
                save_external_people(&ext);
            }
            json_response(request, json!({"ok": true}), 200)
        }
        Err(e) => json_response(request, json!({"ok": false, "error": e.to_string()}), 500),
    }
}

#[cfg(not(feature = "onnx"))]
pub(super) fn handle_voiceprint_learn_sample(
    request: tiny_http::Request,
    _body: serde_json::Value,
) {
    json_response(
        request,
        json!({"ok": false, "error": "Обучение голоса недоступно: сборка без onnx"}),
        501,
    );
}

#[cfg(feature = "onnx")]
pub(super) fn handle_voiceprint_learn_sample(request: tiny_http::Request, body: serde_json::Value) {
    let file = body
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let index = body
        .get("index")
        .and_then(|v| v.as_u64())
        .and_then(|v| usize::try_from(v).ok());
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if file.is_empty() || name.is_empty() || index.is_none() {
        json_response(
            request,
            json!({"ok": false, "error": "Нужны file, index и name"}),
            400,
        );
        return;
    }
    if name.chars().count() > 200 {
        json_response(
            request,
            json!({"ok": false, "error": "Имя слишком длинное"}),
            400,
        );
        return;
    }
    if crate::transcribe::is_transcribe_running() {
        json_response(
            request,
            json!({"ok": false, "error": "Дождитесь окончания расшифровки"}),
            409,
        );
        return;
    }
    let raw = PathBuf::from(&file);
    let audio = if raw.is_absolute() {
        raw
    } else {
        save_dir().join(raw)
    };
    if audio.file_name().is_none() || !inside_save_dir(&audio) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл вне папки записей"}),
            404,
        );
        return;
    }
    let stem = audio
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let json_p = audio
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(save_dir)
        .join(format!("{stem}.transcript.json"));
    let data: serde_json::Value = match std::fs::read_to_string(&json_p) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                log::warn!(
                    target: "json",
                    path = crate::logging::file_token(&json_p),
                    error = error.to_string();
                    "ev=json.load"
                );
                json_response(
                    request,
                    json!({"ok": false, "error": "Расшифровка не найдена"}),
                    404,
                );
                return;
            }
        },
        Err(error) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(&json_p),
                error = error.to_string();
                "ev=json.load"
            );
            json_response(
                request,
                json!({"ok": false, "error": "Расшифровка не найдена"}),
                404,
            );
            return;
        }
    };
    let u = data
        .get("utterances")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(index.unwrap_or(usize::MAX)));
    let (start, end, channel) = match u {
        Some(u) => (
            u.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0),
            u.get("end")
                .and_then(|v| v.as_f64())
                .unwrap_or_else(|| u.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0)),
            u.get("channel").and_then(|v| v.as_i64()).map(|c| c as i32),
        ),
        None => {
            json_response(
                request,
                json!({"ok": false, "error": "Реплика не найдена"}),
                404,
            );
            return;
        }
    };
    let (emb, _clean_dur, quality) =
        match crate::transcribe::extract_utterance_embedding(&audio, start, end, channel) {
            Ok(res) => res,
            Err(e) => {
                json_response(request, json!({"ok": false, "error": e}), 422);
                return;
            }
        };
    if emb.len() != EMBEDDING_DIM {
        json_response(
            request,
            json!({"ok": false, "error": "Неверная размерность голосового отпечатка"}),
            500,
        );
        return;
    }
    let norm = l2_normalize(&emb);
    if norm.iter().all(|x| x.abs() < 1e-12) {
        json_response(
            request,
            json!({"ok": false, "error": "Пустой голосовой отпечаток"}),
            400,
        );
        return;
    }
    let pack_id = body
        .get("pack_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    let chosen_pack = resolve_target_pack(&name, pack_id.as_deref());
    let audio_lossy = audio.to_string_lossy();
    if let Err(error) = store_voiceprint(
        &name,
        Some(&chosen_pack),
        norm,
        PrototypeSample {
            audio_file: &audio_lossy,
            start: Some(start),
            end: Some(end),
            channel: channel.map(|c| c.max(0) as usize),
            quality: Some(quality),
        },
    ) {
        json_response(request, json!({"ok": false, "error": error}), 422);
        return;
    }
    ensure_person_in_pack(&name, Some("Участник"), &chosen_pack);
    json_response(
        request,
        json!({"ok": true, "voiceprint_saved": !emb.is_empty(), "pack_id": chosen_pack}),
        200,
    );
}

pub(super) fn handle_voiceprint_delete(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let audio_file = body.get("audio_file").and_then(|v| v.as_str());
    if name.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Укажите имя участника"}),
            400,
        );
        return;
    }
    let found = delete_voiceprint_internal(name, audio_file);
    json_response(request, json!({"ok": found}), 200);
}

/// Сниппет нового прототипа: образец нужен, чтобы голос можно было узнать на слух.
fn snippet_for_new_prototype(
    audio_file: &str,
    proto_id: &str,
    start: Option<f64>,
    end: Option<f64>,
    channel: Option<usize>,
    pack_dir: Option<&Path>,
) -> Option<String> {
    let (s, e) = match (start, end) {
        (Some(s), Some(e)) if s >= 0.0 && e - s > 0.0 && e - s <= 30.0 => (s, e),
        _ => return None,
    };
    let audio = Path::new(audio_file);
    if !inside_save_dir(audio) || !audio.is_file() {
        return None;
    }
    if !audio
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("wav"))
    {
        return None;
    }
    let bytes = super::media::build_wav_snippet(audio, s, e, channel)?;
    let dir = pack_dir?;
    std::fs::create_dir_all(dir.join("snippets")).ok()?;
    std::fs::write(dir.join("snippets").join(format!("{proto_id}.wav")), bytes).ok()?;
    Some(format!("snippets/{proto_id}.wav"))
}

fn remove_snippet(pack_dir: &Path, rel: &str) {
    if let Some(path) = packs::safe_snippet_path(pack_dir, rel) {
        let _ = std::fs::remove_file(path);
    }
}

fn voiceprint_files() -> Vec<std::path::PathBuf> {
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

pub(super) fn delete_voiceprint_internal(name: &str, audio_file: Option<&str>) -> bool {
    let clean = name.trim();
    if clean.is_empty() {
        return false;
    }
    let mut found = false;
    for vp_file in voiceprint_files() {
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
    if found {
        if let Some(af) = audio_file.filter(|s| !s.is_empty()) {
            clear_matched_in_transcript(Path::new(af), clean);
        }
    }
    found
}

/// pack_id для URL сниппета: "" — корень app_data, иначе имя папки пакета.
fn pack_id_of(vp_file: &Path) -> Option<String> {
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

pub(super) fn serve_voiceprint_detail(request: tiny_http::Request, query: &str) {
    let name = super::query_param(query, "name").unwrap_or_default();
    if name.trim().is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Имя не указано"}),
            400,
        );
        return;
    }
    for vp_file in voiceprint_files() {
        let Ok(t) = std::fs::read_to_string(&vp_file) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
            continue;
        };
        let found = parse_profiles(&v)
            .into_iter()
            .find(|p| same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), &name));
        if let Some(p) = found {
            let pack_id = pack_id_of(&vp_file).unwrap_or_default();
            let protos: Vec<serde_json::Value> = p
                .get("prototypes")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|x| {
                    let rel = x.get("snippet_relpath").and_then(|s| s.as_str());
                    let playable = rel.is_some_and(|r| {
                        let base = if pack_id.is_empty() {
                            app_data_dir()
                        } else {
                            mr_packs::packs_root_dir(&app_data_dir()).join(&pack_id)
                        };
                        packs::safe_snippet_path(&base, r).is_some()
                    });
                    json!({
                        "id": x.get("id"),
                        "created_at": x.get("created_at"),
                        "duration_sec": x.get("duration_sec"),
                        "quality": x.get("quality"),
                        "has_snippet": playable,
                    })
                })
                .collect();
            json_response(
                request,
                json!({
                    "ok": true,
                    "name": p.get("name"),
                    "pack_id": pack_id,
                    "samples_count": p.get("samples_count"),
                    "prototypes": protos,
                }),
                200,
            );
            return;
        }
    }
    json_response(
        request,
        json!({"ok": false, "error": "Голос не найден"}),
        404,
    );
}

/// Удаляет один прототип; центроид — среднее оставшихся, а не старое
/// скользящее среднее. Пустой профиль убирается целиком вместе со сниппетом.
fn remove_prototype(
    profile: &mut serde_json::Value,
    proto_id: &str,
) -> Option<(Option<String>, bool)> {
    let arr = profile.get_mut("prototypes")?.as_array_mut()?;
    let pos = arr
        .iter()
        .position(|x| x.get("id").and_then(|v| v.as_str()) == Some(proto_id))?;
    let gone = arr.remove(pos);
    let snippet = gone
        .get("snippet_relpath")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let (centroid, count) = prototypes_centroid(arr);
    if count == 0 {
        return Some((snippet, true));
    }
    profile["embedding"] = json!(centroid);
    profile["samples_count"] = json!(count as u64);
    Some((snippet, false))
}

pub(super) fn handle_voiceprint_sample_delete(
    request: tiny_http::Request,
    body: serde_json::Value,
) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let proto_id = body
        .get("prototype_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() || proto_id.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Нужно имя и id образца"}),
            400,
        );
        return;
    }
    let requested_pack = body.get("pack_id").and_then(|v| v.as_str());
    let all_files = voiceprint_files();
    let files: Vec<PathBuf> = match requested_pack {
        Some(id) => all_files
            .into_iter()
            .filter(|f| {
                let pid = pack_id_of(f).unwrap_or_default();
                let canon = if pid.is_empty() { "default" } else { &pid };
                canon == id || pid == id
            })
            .collect(),
        None => all_files,
    };
    let _guard = lock_voiceprints();
    // Один и тот же id прототипа в разных пакетах (например, из импорта) нельзя
    // удалять «везде»: выбор пакета остаётся за пользователем.
    if requested_pack.is_none() {
        let mut pack_ids: Vec<String> = Vec::new();
        for vp_file in &files {
            let Ok(t) = std::fs::read_to_string(vp_file) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
                continue;
            };
            let hit = parse_profiles(&v).iter().any(|p| {
                same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), &name)
                    && p.get("prototypes")
                        .and_then(|x| x.as_array())
                        .is_some_and(|a| {
                            a.iter().any(|x| {
                                x.get("id").and_then(|v| v.as_str()) == Some(proto_id.as_str())
                            })
                        })
            });
            if hit {
                if let Some(id) = pack_id_of(vp_file) {
                    let canon_id = if id.is_empty() {
                        "default".to_string()
                    } else {
                        id
                    };
                    if !pack_ids.contains(&canon_id) {
                        pack_ids.push(canon_id);
                    }
                }
            }
        }
        if pack_ids.len() > 1 {
            json_response(
                request,
                json!({
                    "ok": false,
                    "error": "Образец найден в нескольких пакетах — укажите pack_id",
                    "pack_ids": pack_ids,
                }),
                409,
            );
            return;
        }
    }
    let mut found = false;
    for vp_file in files {
        let Ok(t) = std::fs::read_to_string(&vp_file) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else {
            continue;
        };
        let mut profiles = parse_profiles(&v);
        let mut changed = false;
        let mut drop_profile = false;
        let mut snippet: Option<String> = None;
        if let Some(p) = profiles
            .iter_mut()
            .find(|p| same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), &name))
        {
            if let Some((sn, empty)) = remove_prototype(p, &proto_id) {
                snippet = sn;
                drop_profile = empty;
                changed = true;
                found = true;
            }
        }
        if drop_profile {
            profiles.retain(|p| {
                !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), &name)
            });
        }
        if changed {
            let _ = write_atomic(
                &vp_file,
                serde_json::to_string_pretty(&vp_v3(profiles))
                    .unwrap_or_default()
                    .as_bytes(),
            );
            if let (Some(rel), Some(parent)) = (snippet, vp_file.parent()) {
                remove_snippet(parent, &rel);
            }
        }
    }
    json_response(request, json!({"ok": found}), 200);
}

/// Удалённый голос больше не должен показываться как «узнанный» в открытом
/// transcript.json: снимаем matched-флаг, имя кластера оставляем для контекста.
fn clear_matched_in_transcript(audio: &Path, name: &str) {
    if !inside_save_dir(audio) {
        return;
    }
    let stem = audio
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = audio
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(save_dir);
    let json_p = parent.join(format!("{stem}.transcript.json"));
    let Ok(t) = std::fs::read_to_string(&json_p) else {
        return;
    };
    let Ok(mut data) = serde_json::from_str::<serde_json::Value>(&t) else {
        return;
    };
    let mut changed = false;
    if let Some(arr) = data
        .get_mut("meta")
        .and_then(|m| m.get_mut("speakers_info"))
        .and_then(|a| a.as_array_mut())
    {
        for spk in arr.iter_mut() {
            let hit = spk
                .get("matched_name")
                .and_then(|x| x.as_str())
                .map(|n| same_name(n, name))
                .unwrap_or(false)
                || spk
                    .get("name")
                    .and_then(|x| x.as_str())
                    .map(|n| same_name(n, name))
                    .unwrap_or(false);
            if hit && spk.get("matched").and_then(|x| x.as_bool()) == Some(true) {
                spk["matched"] = json!(false);
                spk["matched_name"] = serde_json::Value::Null;
                changed = true;
            }
        }
    }
    if changed {
        let _ = write_atomic(
            &json_p,
            serde_json::to_string_pretty(&data)
                .unwrap_or_default()
                .as_bytes(),
        );
    }
}

fn rename_speaker_in_files(audio: &Path, new_name: &str, old_name: Option<&str>) {
    let stem = audio
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = audio
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(save_dir);
    let json_p = parent.join(format!("{stem}.transcript.json"));
    let md_p = parent.join(format!("{stem}.md"));
    let txt_p = parent.join(format!("{stem}.txt"));
    let mut old = old_name.map(|s| s.to_string());
    let mut unique_speakers = Vec::new();
    if let Ok(t) = std::fs::read_to_string(&json_p) {
        if let Ok(mut data) = serde_json::from_str::<serde_json::Value>(&t) {
            if let Some(meta) = data.get_mut("meta") {
                if let Some(arr) = meta.get_mut("speakers_info").and_then(|v| v.as_array_mut()) {
                    for spk in arr.iter_mut() {
                        let cur = spk
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let hit = match &old {
                            Some(o) => same_name(&cur, o),
                            None => true,
                        };
                        if hit {
                            if old.is_none() {
                                old = Some(cur);
                            }
                            spk["name"] = json!(new_name);
                            spk["matched"] = json!(true);
                            spk["matched_name"] = json!(new_name);
                        }
                    }
                }
                if let Some(list) = meta.get_mut("speakers").and_then(|v| v.as_array_mut()) {
                    if let Some(o) = old.as_deref() {
                        for s in list.iter_mut() {
                            if s.as_str().map(|n| same_name(n, o)).unwrap_or(false) {
                                *s = json!(new_name);
                            }
                        }
                    }
                    let mut deduped = Vec::new();
                    for s in list.drain(..) {
                        if !deduped.contains(&s) {
                            deduped.push(s);
                        }
                    }
                    *list = deduped;
                }
                if let Some(list) = meta.get("speakers").and_then(|v| v.as_array()) {
                    for s in list {
                        if let Some(name) = s.as_str() {
                            if !unique_speakers.contains(&name.to_string()) {
                                unique_speakers.push(name.to_string());
                            }
                        }
                    }
                }
            }
            if let Some(arr) = data.get_mut("utterances").and_then(|v| v.as_array_mut()) {
                if let Some(o) = &old {
                    for u in arr.iter_mut() {
                        if u.get("speaker")
                            .and_then(|x| x.as_str())
                            .map(|n| same_name(n, o))
                            .unwrap_or(false)
                        {
                            u["speaker"] = json!(new_name);
                        }
                    }
                }
            }
            let _ = write_atomic(
                &json_p,
                serde_json::to_string_pretty(&data)
                    .unwrap_or_default()
                    .as_bytes(),
            );
        }
    }
    if let Some(o) = &old {
        if let Ok(t) = std::fs::read_to_string(&md_p) {
            let mut patched = t.replace(&format!("] {o}:"), &format!("] {new_name}:"));
            if !unique_speakers.is_empty() {
                let lines: Vec<String> = patched
                    .lines()
                    .map(|line| {
                        if line.starts_with("**Спикеры:**") {
                            format!("**Спикеры:** {}", unique_speakers.join(", "))
                        } else {
                            line.to_string()
                        }
                    })
                    .collect();
                patched = lines.join("\n");
                if t.ends_with('\n') {
                    patched.push('\n');
                }
            }
            if patched != t {
                let _ = write_atomic(&md_p, patched.as_bytes());
            }
        }
        if let Ok(t) = std::fs::read_to_string(&txt_p) {
            let patched = t.replace(&format!("] {o}:"), &format!("] {new_name}:"));
            if patched != t {
                let _ = write_atomic(&txt_p, patched.as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(two: bool) -> serde_json::Value {
        let a: Vec<f64> = (0..EMBEDDING_DIM).map(|_| 1.0).collect();
        let b: Vec<f64> = (0..EMBEDDING_DIM).map(|_| 0.5).collect();
        let mut protos = vec![json!({
            "id": "p1", "embedding": a, "duration_sec": 1.0,
            "quality": 1.0, "created_at": "", "snippet_relpath": null,
        })];
        if two {
            protos.push(json!({
                "id": "p2", "embedding": b, "duration_sec": 2.0,
                "quality": 1.0, "created_at": "", "snippet_relpath": null,
            }));
        }
        json!({"name": "Тест-Голос", "embedding": a_fallback(), "samples_count": 2, "prototypes": protos})
    }

    fn a_fallback() -> Vec<f64> {
        (0..EMBEDDING_DIM).map(|_| 0.1).collect()
    }

    #[test]
    fn remove_prototype_recomputes_centroid() {
        let mut p = profile_with(true);
        let (sn, empty) = remove_prototype(&mut p, "p1").expect("p1 обязан найтись");
        assert!(sn.is_none());
        assert!(!empty);
        assert_eq!(p["samples_count"], 1);
        let emb: Vec<f64> = serde_json::from_value(p["embedding"].clone()).unwrap();
        assert_eq!(emb.len(), EMBEDDING_DIM);
        // Остался один прототип из 0.5: центроид — его нормаль, а не старое среднее.
        let norm: f64 = emb.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 1e-9);
        assert!((emb[0] - emb[100]).abs() < 1e-12);
    }

    #[test]
    fn store_voiceprint_centroid_follows_retained_prototypes() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "mr-voiceprint-centroid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

        let name = "Тест-Дрейф";
        for i in 0..15 {
            let mut v = vec![1.0; EMBEDDING_DIM];
            v[i] = 20.0;
            store_voiceprint(
                name,
                Some("default"),
                l2_normalize(&v),
                PrototypeSample {
                    start: Some(1.0),
                    end: Some(3.0),
                    ..Default::default()
                },
            )
            .expect("прототип сохраняется");
        }
        let vp = load_all_voiceprints()
            .into_iter()
            .find(|v| same_name(v.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
            .expect("профиль сохранён");
        let protos = vp["prototypes"].as_array().expect("prototypes");
        assert_eq!(protos.len(), MAX_PROTOTYPES);
        assert_eq!(vp["samples_count"], MAX_PROTOTYPES as u64);
        assert!((protos[0]["duration_sec"].as_f64().unwrap() - 2.0).abs() < 1e-9);
        let stored: Vec<f64> = serde_json::from_value(vp["embedding"].clone()).unwrap();
        let (expected, count) = prototypes_centroid(protos);
        assert_eq!(count, MAX_PROTOTYPES);
        for (a, b) in stored.iter().zip(expected.iter()) {
            assert!((a - b).abs() < 1e-12, "центроид не совпал: {a} vs {b}");
        }

        if let Some(p) = prev {
            std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
        } else {
            std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snippet_path_never_escapes_pack() {
        let dir = std::env::temp_dir().join(format!(
            "mr-snippet-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("snippets")).unwrap();
        std::fs::write(dir.join("snippets/a.wav"), b"x").unwrap();
        assert!(packs::safe_snippet_path(&dir, "snippets/a.wav").is_some());
        assert!(packs::safe_snippet_path(&dir, "../a.wav").is_none());
        assert!(packs::safe_snippet_path(&dir, "/etc/passwd").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn voiceprint_outlier_is_rejected() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "mr-voiceprint-outlier-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);
        let name = "Тест-Выброс";
        for i in 0..3 {
            let mut v = vec![0.0; EMBEDDING_DIM];
            v[i] = 1.0;
            store_voiceprint(name, Some("default"), v, PrototypeSample::default())
                .expect("свой прототип");
        }
        let mut alien = vec![0.0; EMBEDDING_DIM];
        alien[100] = 1.0;
        let error = store_voiceprint(name, Some("default"), alien, PrototypeSample::default())
            .expect_err("чужой голос должен быть отклонён");
        assert!(error.contains("другой голос"), "{error}");
        let vp = load_all_voiceprints()
            .into_iter()
            .find(|v| same_name(v.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
            .expect("профиль");
        assert_eq!(vp["prototypes"].as_array().map(Vec::len), Some(3));

        if let Some(p) = prev {
            std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
        } else {
            std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_last_prototype_empties_profile() {
        let mut p = profile_with(false);
        let (_, empty) = remove_prototype(&mut p, "p1").expect("p1 обязан найтись");
        assert!(empty);
        assert!(remove_prototype(&mut p, "nope").is_none());
    }

    #[test]
    fn target_pack_routing_and_people_sync() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "mr-pack-routing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

        let custom = mr_packs::create_pack(&dir, "Команда Альфа", "").unwrap();
        ensure_person_in_pack("Инженер 1", Some("Разработчик"), &custom.id);

        assert_eq!(resolve_target_pack("Инженер 1", None), custom.id);
        assert_eq!(resolve_target_pack("Новый", None), "default");
        assert_eq!(resolve_target_pack("Новый", Some(&custom.id)), custom.id);
        assert_eq!(
            resolve_target_pack("Новый", Some(&dir.to_string_lossy())),
            "default"
        );
        assert_eq!(resolve_target_pack("Новый", Some("../evil")), "default");

        let custom2 = mr_packs::create_pack(&dir, "Команда Бета", "").unwrap();
        let ok = update_people(
            "Инженер 1",
            "Инженер 1 (Старший)",
            Some("Лид"),
            Some(&custom2.id),
            Some(&custom.id),
        );
        assert!(ok);
        assert_eq!(resolve_target_pack("Инженер 1 (Старший)", None), custom2.id);
        let p1_file = dir.join("packs").join(&custom.id).join("people.json");
        let t1 = std::fs::read_to_string(&p1_file).unwrap_or_default();
        assert!(!t1.contains("Инженер 1"));
        let p2_file = dir.join("packs").join(&custom2.id).join("people.json");
        let t2 = std::fs::read_to_string(&p2_file).unwrap_or_default();
        assert!(t2.contains("Инженер 1 (Старший)"));
        assert!(t2.contains("Лид"));

        if let Some(p) = prev {
            std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
        } else {
            std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_person_deletes_voiceprints_and_prevents_resurrection() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "mr-remove-person-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

        let person_name = "Участник 1";
        ensure_person_in_pack(person_name, Some("Спикер"), "default");
        let emb = l2_normalize(
            &(0..EMBEDDING_DIM)
                .map(|i| (i + 1) as f64)
                .collect::<Vec<_>>(),
        );
        store_voiceprint(
            person_name,
            Some("default"),
            emb,
            PrototypeSample::default(),
        )
        .expect("голос сохраняется");

        let vps_before = load_all_voiceprints();
        assert!(vps_before.iter().any(|v| same_name(
            v.get("name").and_then(|x| x.as_str()).unwrap_or(""),
            person_name
        )));

        let ok = remove_person(person_name);
        assert!(ok);

        let vps_after = load_all_voiceprints();
        assert!(!vps_after.iter().any(|v| same_name(
            v.get("name").and_then(|x| x.as_str()).unwrap_or(""),
            person_name
        )));

        let p_file = dir.join("packs/default/people.json");
        let text = std::fs::read_to_string(&p_file).unwrap_or_default();
        assert!(!text.contains(person_name));

        if let Some(p) = prev {
            std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
        } else {
            std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_voiceprint_evicts_lowest_quality_prototype() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "mr-voiceprint-evict-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

        let name = "Тест-Вытеснение";
        for i in 0..10 {
            let mut v = vec![1.0; EMBEDDING_DIM];
            v[i] = 10.0;
            store_voiceprint(
                name,
                Some("default"),
                l2_normalize(&v),
                PrototypeSample {
                    start: Some(0.0),
                    end: Some(4.0),
                    quality: Some(1.0),
                    ..Default::default()
                },
            )
            .expect("прототип");
        }
        let mut low = vec![1.0; EMBEDDING_DIM];
        low[10] = 10.0;
        store_voiceprint(
            name,
            Some("default"),
            l2_normalize(&low),
            PrototypeSample {
                start: Some(0.0),
                end: Some(1.0),
                quality: Some(0.3),
                ..Default::default()
            },
        )
        .expect("слабый прототип");

        let vp = load_all_voiceprints()
            .into_iter()
            .find(|v| same_name(v.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
            .expect("профиль");
        let protos = vp["prototypes"].as_array().expect("prototypes");
        assert_eq!(protos.len(), MAX_PROTOTYPES);
        assert!(
            protos.iter().all(|p| {
                let q = p["quality"].as_f64().unwrap_or(0.0);
                (q - 1.0).abs() < 1e-6
            }),
            "слабый прототип должен быть вытеснен первым"
        );

        if let Some(p) = prev {
            std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
        } else {
            std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
