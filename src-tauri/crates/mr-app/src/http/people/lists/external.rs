use std::path::{Path, PathBuf};

use mr_packs as packs;
use serde_json::json;

use super::super::transcript_names::rename_voiceprint_names;
use super::super::{lock_people, lock_voiceprints};
use super::packs::{people_json_files, resolve_target_pack};
use crate::config::app_data_dir;
use crate::fsutil::write_atomic;

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
            let move_res = {
                let _vp = lock_voiceprints();
                packs::move_person_pack(&app_data_dir(), old, &from_pack, target_id)
            };
            if let Err(e) = move_res {
                crate::note!(
                    "[warn] move_person_pack {old} from {from_pack} to {target_id}: {e:?}"
                );
                // Ошибка переноса прерывает операцию: иначе очистка дублей ниже удалит контакт везде.
                return false;
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
