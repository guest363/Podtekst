use std::path::{Component, Path, PathBuf};

use mr_packs as packs;
use serde_json::json;

use super::transcript_names::rename_voiceprint_names;
use super::voiceprints::{
    delete_voiceprint_files, delete_voiceprint_internal, load_all_voiceprints,
    pack_voiceprint_file, parse_profiles, vp_v3,
};
use super::{lock_people, lock_voiceprints};
use crate::config::app_data_dir;
use crate::fsutil::write_atomic;
use crate::http::json_response;
use crate::voiceprints::{EMBEDDING_DIM, EMBEDDING_SPACE};
fn external_people_path() -> PathBuf {
    app_data_dir().join("external_people.json")
}

pub(crate) fn load_external_people() -> Vec<serde_json::Value> {
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

pub(crate) fn save_external_people(list: &[serde_json::Value]) {
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

pub(crate) fn read_people_list(path: &Path) -> Option<Vec<serde_json::Value>> {
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

pub(crate) fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

pub(crate) fn update_people(
    old: &str,
    new_name: &str,
    role: Option<&str>,
    target_pack_id: Option<&str>,
    from_pack_id: Option<&str>,
) -> bool {
    let _people = lock_people();
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
/// `pack_id: Some(id)` — только выбранный пакет: имя уникально внутри пакета,
/// и одноимённый участник в другом пакете это другой человек.
pub(crate) fn remove_person(name: &str, pack_id: Option<&str>) -> bool {
    let _people = lock_people();
    if let Some(pack_id) = pack_id.map(str::trim).filter(|s| !s.is_empty()) {
        return remove_person_from_one_pack(name, pack_id);
    }
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

fn remove_person_from_one_pack(name: &str, pack_id: &str) -> bool {
    let removed = match packs::remove_person_from_pack(&app_data_dir(), pack_id, name) {
        Ok(removed) => removed,
        Err(error) => {
            log::warn!(target: "json", error = error.to_string(); "ev=people.remove_pack");
            return false;
        }
    };
    let vp_found = delete_voiceprint_files(name, vec![pack_voiceprint_file(pack_id)]);
    removed || vp_found
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
    let _people = lock_people();
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

pub(crate) fn handle_people_add(request: tiny_http::Request, body: serde_json::Value) {
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

pub(crate) fn handle_people_move(request: tiny_http::Request, body: serde_json::Value) {
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
