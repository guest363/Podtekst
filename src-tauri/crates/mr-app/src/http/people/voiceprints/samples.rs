use std::path::PathBuf;

use mr_packs as packs;
use serde_json::json;

use super::super::lists::same_name;
use super::super::lock_voiceprints;
use super::prototypes_centroid;
use super::snippets::remove_snippet;
use super::store::{pack_id_of, parse_profiles, voiceprint_files, vp_v3};
use crate::config::app_data_dir;
use crate::fsutil::write_atomic;
use crate::http::{json_response, query_param};

pub(crate) fn serve_voiceprint_detail(request: tiny_http::Request, query: &str) {
    let name = query_param(query, "name").unwrap_or_default();
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
pub(crate) fn remove_prototype(
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

pub(crate) fn handle_voiceprint_sample_delete(
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
