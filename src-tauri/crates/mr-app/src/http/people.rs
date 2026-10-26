//! Участники, гости и голосовые отпечатки (Voice Packs + external_people.json).

use std::path::{Path, PathBuf};

use mr_packs as packs;
use serde_json::json;

use super::json_response;
use super::media::inside_save_dir;
use crate::config::{app_data_dir, save_dir};

fn external_people_path() -> PathBuf {
    app_data_dir().join("external_people.json")
}

pub(super) fn load_external_people() -> Vec<serde_json::Value> {
    std::fs::read_to_string(external_people_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

pub(super) fn save_external_people(list: &[serde_json::Value]) {
    let _ = std::fs::create_dir_all(app_data_dir());
    let _ = std::fs::write(
        external_people_path(),
        serde_json::to_string_pretty(&json!(list)).unwrap_or_default(),
    );
}

/// Люди из активных голосовых пакетов (как отдаёт `/api/people`).
pub(super) fn people_json_files() -> Vec<PathBuf> {
    packs::get_active_packs(&app_data_dir())
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.join("people.json"))
        .collect()
}

/// Профили из voiceprints.json любой схемы: массив (legacy) или v2/v3 `{"profiles": []}`.
fn parse_profiles(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.get("profiles")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_else(|| v.as_array().cloned().unwrap_or_default())
}

/// Канонический объект voiceprints.json v3: пространство `eres2netv2-192-v1`.
fn vp_v3(profiles: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"version": 3, "embedding_space": mr_ml::EMBEDDING_SPACE, "profiles": profiles})
}

/// Разовая миграция при старте: профили старых пространств (pyannote-256-v1 и
/// прочие размерности ≠ текущей) не пересчитать без исходных сэмплов — удаляем
/// и начинаем собирать заново; оригинал сохраняется в `voiceprints.v2.bak`.
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
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let profiles = parse_profiles(&v);
        let n_before = profiles.len();
        let space_ok = v
            .get("embedding_space")
            .and_then(|x| x.as_str())
            .map(|s| s == mr_ml::EMBEDDING_SPACE)
            .unwrap_or(true);
        let kept: Vec<_> = if space_ok {
            profiles
                .into_iter()
                .filter(|p| {
                    p.get("embedding")
                        .and_then(|x| x.as_array())
                        .map(|a| a.len() == mr_ml::EMBEDDING_DIM)
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
        let _ = std::fs::write(
            &file,
            serde_json::to_string_pretty(&vp_v3(kept)).unwrap_or_default(),
        );
    }
}

pub(super) fn read_people_list(path: &Path) -> Option<Vec<serde_json::Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
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
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                let mut profiles = v
                    .get("profiles")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_else(|| v.as_array().cloned().unwrap_or_default());
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
                    let _ = std::fs::write(
                        &file,
                        serde_json::to_string_pretty(&vp_v3(profiles)).unwrap_or_default(),
                    );
                }
            }
        }
    }
    found
}

/// Обновляет имя/роль человека в пакетах и среди гостей.
pub(super) fn update_people(old: &str, new_name: &str, role: Option<&str>) -> bool {
    let mut found = false;
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
            let _ = std::fs::write(
                &file,
                serde_json::to_string_pretty(&list).unwrap_or_default(),
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

/// Убирает человека из пакетов и списка гостей. Голосовой отпечаток остаётся:
/// удалить голос отдельно можно в «Текст и спикеры» → «Это не тот голос».
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
            let _ = std::fs::write(
                &file,
                serde_json::to_string_pretty(&kept).unwrap_or_default(),
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
    found
}

/// Профиль пригоден для текущего embedding-пространства: есть центроид или
/// хотя бы один прототип нужной размерности.
fn voiceprint_usable(p: &serde_json::Value) -> bool {
    let emb_ok = p
        .get("embedding")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.len() == mr_ml::EMBEDDING_DIM
                && a.iter().filter_map(|x| x.as_f64()).any(|x| x.abs() > 1e-12)
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
                        e.len() == mr_ml::EMBEDDING_DIM
                            && e.iter().filter_map(|n| n.as_f64()).any(|n| n.abs() > 1e-12)
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    emb_ok || proto_ok
}

/// Все голосовые профили: активные пакеты Voice Packs (+ локальный файл при наличии).
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
            if let Ok(t) = std::fs::read_to_string(dir.join("voiceprints.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("embedding_space")
                        .and_then(|x| x.as_str())
                        .is_some_and(|s| s != mr_ml::EMBEDDING_SPACE)
                    {
                        continue;
                    }
                    for p in parse_profiles(&v) {
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
                    .is_some_and(|s| s != mr_ml::EMBEDDING_SPACE)
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

fn l2_normalize(v: &[f64]) -> Vec<f64> {
    let n: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if n < 1e-12 {
        return vec![0.0; v.len()];
    }
    v.iter().map(|x| x / n).collect()
}

/// Гарантирует, что человек с указанным именем присутствует либо в одном из
/// активных голосовых пакетов, либо в `external_people.json`. Если человека нет
/// нигде, он добавляется в `external_people.json` с указанной ролью (по умолчанию "Участник").
pub(super) fn ensure_person_in_list(name: &str, role: Option<&str>) {
    let clean = name.trim();
    if clean.is_empty() {
        return;
    }
    let in_packs = people_json_files().iter().any(|pf| {
        read_people_list(pf).unwrap_or_default().iter().any(|p| {
            p.get("name")
                .and_then(|x| x.as_str())
                .map(|n| same_name(n, clean))
                .unwrap_or(false)
        })
    });
    let mut ext = load_external_people();
    let in_ext = ext.iter().any(|p| {
        p.get("name")
            .and_then(|x| x.as_str())
            .map(|n| same_name(n, clean))
            .unwrap_or(false)
    });
    if !in_packs && !in_ext {
        ext.push(json!({
            "name": clean,
            "role": role.unwrap_or("Участник"),
            "is_external": true,
        }));
        save_external_people(&ext);
    }
}

/// Синхронизация профилей из `voiceprints.json`: если у человека есть сохранённый отпечаток,
/// но он отсутствует в `people.json` и `external_people.json`, добавляем его в список участников.
pub(crate) fn sync_voiceprint_people() {
    for vp in load_all_voiceprints() {
        if let Some(name) = vp.get("name").and_then(|x| x.as_str()) {
            ensure_person_in_list(name, Some("Участник"));
        }
    }
}

/// POST /api/voiceprints/save — сохраняет профиль голоса в основной пакет Voice Packs.
pub(super) fn handle_voiceprint_save(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let emb: Vec<f64> = body
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
    if name.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Имя не может быть пустым"}),
            400,
        );
        return;
    }
    if !emb.is_empty() && emb.len() != mr_ml::EMBEDDING_DIM {
        json_response(
            request,
            json!({"ok": false, "error": "Неверная размерность голосового отпечатка"}),
            400,
        );
        return;
    }
    // Если передан вектор голоса — сохраняем/обновляем в канонический default-пак
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
        let now = crate::recording::now_secs();
        let vp_file = packs::ensure_default_pack(&app_data_dir())
            .map(|d| d.join("voiceprints.json"))
            .unwrap_or_else(|_| app_data_dir().join("packs/default/voiceprints.json"));
        if let Some(parent) = vp_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut profiles: Vec<serde_json::Value> = std::fs::read_to_string(&vp_file)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| {
                if v.get("embedding_space")
                    .and_then(|x| x.as_str())
                    .is_some_and(|s| s != mr_ml::EMBEDDING_SPACE)
                {
                    vec![]
                } else {
                    parse_profiles(&v)
                }
            })
            .unwrap_or_default();
        profiles.retain(voiceprint_usable);
        if let Some(p) = profiles.iter_mut().find(|p| {
            p.get("name")
                .and_then(|x| x.as_str())
                .map(|n| same_name(n, &name))
                .unwrap_or(false)
        }) {
            let old: Vec<f64> = p
                .get("embedding")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
                .unwrap_or_default();
            let cnt = p.get("samples_count").and_then(|x| x.as_u64()).unwrap_or(1) as f64;
            let merged = if old.len() == norm.len() {
                l2_normalize(
                    &old.iter()
                        .zip(norm.iter())
                        .map(|(o, n)| (o * cnt + n) / (cnt + 1.0))
                        .collect::<Vec<_>>(),
                )
            } else {
                norm.clone()
            };
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
            let duplicate = protos.iter().any(|x| {
                x.get("embedding")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        let v: Vec<f64> = a.iter().filter_map(|n| n.as_f64()).collect();
                        mr_ml::cosine_similarity(&v, &norm) >= 0.98
                    })
                    .unwrap_or(false)
            });
            if !duplicate {
                protos.push(json!({
                    "id": format!("p{now}-{}", protos.len() + 1),
                    "embedding": norm,
                    "duration_sec": 4.0,
                    "quality": 1.0,
                    "created_at": now,
                    "snippet_relpath": null,
                }));
                p["embedding"] = json!(merged);
                p["samples_count"] = json!(cnt as u64 + 1);
            }
            if protos.len() > mr_ml::MAX_PROTOTYPES {
                let drop_n = protos.len() - mr_ml::MAX_PROTOTYPES;
                protos.drain(0..drop_n);
            }
            p["name"] = json!(name.clone());
            p["prototypes"] = json!(protos);
            p["updated_at"] = json!(now);
        } else {
            profiles.push(json!({
                "name": name.clone(),
                "embedding": norm.clone(),
                "samples_count": 1,
                "updated_at": now,
                "prototypes": [{
                    "id": format!("p{now}-1"),
                    "embedding": norm,
                    "duration_sec": 4.0,
                    "quality": 1.0,
                    "created_at": now,
                    "snippet_relpath": null,
                }],
            }));
        }
        let _ = std::fs::write(
            &vp_file,
            serde_json::to_string_pretty(&vp_v3(profiles)).unwrap_or_default(),
        );
    }

    // Человек гарантированно добавляется в «Участники встречи»
    ensure_person_in_list(&name, Some("Участник"));
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
    json_response(request, json!({"ok": true}), 200);
}

/// POST /api/voiceprints/delete
pub(super) fn handle_voiceprint_delete(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let audio_file = body
        .get("audio_file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Имя не может быть пустым"}),
            400,
        );
        return;
    }
    let mut files: Vec<std::path::PathBuf> = vec![
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
    let mut found = false;
    for vp_file in files {
        if let Ok(t) = std::fs::read_to_string(&vp_file) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                let arr = parse_profiles(&v);
                let before = arr.len();
                let kept: Vec<_> = arr
                    .into_iter()
                    .filter(|p| {
                        p.get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_lowercase()
                            != name
                    })
                    .collect();
                if kept.len() != before {
                    found = true;
                    let _ = std::fs::write(
                        &vp_file,
                        serde_json::to_string_pretty(&vp_v3(kept)).unwrap_or_default(),
                    );
                }
            }
        }
    }
    if found && !audio_file.is_empty() {
        clear_matched_in_transcript(Path::new(&audio_file), &name);
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
        let _ = std::fs::write(
            &json_p,
            serde_json::to_string_pretty(&data).unwrap_or_default(),
        );
    }
}

/// Переименование спикера в .transcript.json + точечная правка .md/.txt (fallback без форматтеров).
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
            let _ = std::fs::write(
                &json_p,
                serde_json::to_string_pretty(&data).unwrap_or_default(),
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
                let _ = std::fs::write(&md_p, patched);
            }
        }
        if let Ok(t) = std::fs::read_to_string(&txt_p) {
            let patched = t.replace(&format!("] {o}:"), &format!("] {new_name}:"));
            if patched != t {
                let _ = std::fs::write(&txt_p, patched);
            }
        }
    }
}
