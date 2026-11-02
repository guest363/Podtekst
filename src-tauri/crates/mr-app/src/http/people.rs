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

fn vp_v3(profiles: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"version": 3, "embedding_space": mr_ml::EMBEDDING_SPACE, "profiles": profiles})
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

pub(crate) fn sync_voiceprint_people() {
    for vp in load_all_voiceprints() {
        if let Some(name) = vp.get("name").and_then(|x| x.as_str()) {
            ensure_person_in_list(name, Some("Участник"));
        }
    }
}

fn store_voiceprint(
    name: &str,
    norm: Vec<f64>,
    audio_file: &str,
    sample_start: Option<f64>,
    sample_end: Option<f64>,
    sample_channel: Option<usize>,
) {
    let now = crate::recording::now_secs();
    let now_iso = chrono::Utc::now().to_rfc3339();
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
            .map(|n| same_name(n, name))
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
            let new_id = format!("p{now}-{}", protos.len() + 1);
            let snippet_rel = snippet_for_new_prototype(
                audio_file,
                &new_id,
                sample_start,
                sample_end,
                sample_channel,
                vp_file.parent(),
            );
            protos.push(json!({
                "id": new_id,
                "embedding": norm,
                "duration_sec": 4.0,
                "quality": 1.0,
                "created_at": now_iso.clone(),
                "snippet_relpath": snippet_rel,
            }));
            p["embedding"] = json!(merged);
            p["samples_count"] = json!(cnt as u64 + 1);
        }
        if protos.len() > mr_ml::MAX_PROTOTYPES {
            let drop_n = protos.len() - mr_ml::MAX_PROTOTYPES;
            protos.drain(0..drop_n);
        }
        p["name"] = json!(name);
        p["prototypes"] = json!(protos);
        p["updated_at"] = json!(now_iso.clone());
    } else {
        let new_id = format!("p{now}-1");
        let snippet_rel = snippet_for_new_prototype(
            audio_file,
            &new_id,
            sample_start,
            sample_end,
            sample_channel,
            vp_file.parent(),
        );
        profiles.push(json!({
            "name": name,
            "embedding": norm.clone(),
            "samples_count": 1,
            "updated_at": now_iso.clone(),
            "prototypes": [{
                "id": new_id,
                "embedding": norm,
                "duration_sec": 4.0,
                "quality": 1.0,
                "created_at": now_iso,
                "snippet_relpath": snippet_rel,
            }],
        }));
    }
    let _ = std::fs::write(
        &vp_file,
        serde_json::to_string_pretty(&vp_v3(profiles)).unwrap_or_default(),
    );
}

pub(super) fn handle_voiceprint_save(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
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
                if let Ok(e) =
                    crate::transcribe::extract_utterance_embedding(&full_p, start, end, channel_i32)
                {
                    emb = e;
                }
            }
        }
    }
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
        store_voiceprint(
            &name,
            norm,
            &audio_file,
            sample_start,
            sample_end,
            sample_channel,
        );
    }

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
    json_response(
        request,
        json!({"ok": true, "voiceprint_saved": !emb.is_empty()}),
        200,
    );
}

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
    let data: serde_json::Value = match std::fs::read_to_string(&json_p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
    {
        Some(v) => v,
        None => {
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
    let emb = match crate::transcribe::extract_utterance_embedding(&audio, start, end, channel) {
        Ok(e) => e,
        Err(e) => {
            json_response(request, json!({"ok": false, "error": e}), 500);
            return;
        }
    };
    if emb.len() != mr_ml::EMBEDDING_DIM {
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
    store_voiceprint(
        &name,
        norm,
        &audio.to_string_lossy(),
        Some(start),
        Some(end),
        channel.map(|c| c.max(0) as usize),
    );
    ensure_person_in_list(&name, Some("Участник"));
    json_response(
        request,
        json!({"ok": true, "voiceprint_saved": !emb.is_empty()}),
        200,
    );
}

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
                        base.join(r).is_file()
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
    let remaining: Vec<Vec<f64>> = arr
        .iter()
        .filter_map(|x| {
            x.get("embedding")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|n| n.as_f64()).collect())
        })
        .filter(|v: &Vec<f64>| v.len() == mr_ml::EMBEDDING_DIM)
        .collect();
    if remaining.is_empty() {
        return Some((snippet, true));
    }
    let mut mean = vec![0.0; mr_ml::EMBEDDING_DIM];
    for v in &remaining {
        for (i, x) in v.iter().enumerate() {
            mean[i] += x;
        }
    }
    let n = remaining.len() as f64;
    for x in mean.iter_mut() {
        *x /= n;
    }
    profile["embedding"] = json!(l2_normalize(&mean));
    profile["samples_count"] = json!(remaining.len() as u64);
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
    let mut found = false;
    for vp_file in voiceprint_files() {
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
            let _ = std::fs::write(
                &vp_file,
                serde_json::to_string_pretty(&vp_v3(profiles)).unwrap_or_default(),
            );
            if let (Some(rel), Some(parent)) = (snippet, vp_file.parent()) {
                let _ = std::fs::remove_file(parent.join(&rel));
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
        let _ = std::fs::write(
            &json_p,
            serde_json::to_string_pretty(&data).unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with(two: bool) -> serde_json::Value {
        let a: Vec<f64> = (0..mr_ml::EMBEDDING_DIM).map(|_| 1.0).collect();
        let b: Vec<f64> = (0..mr_ml::EMBEDDING_DIM).map(|_| 0.5).collect();
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
        (0..mr_ml::EMBEDDING_DIM).map(|_| 0.1).collect()
    }

    #[test]
    fn remove_prototype_recomputes_centroid() {
        let mut p = profile_with(true);
        let (sn, empty) = remove_prototype(&mut p, "p1").expect("p1 обязан найтись");
        assert!(sn.is_none());
        assert!(!empty);
        assert_eq!(p["samples_count"], 1);
        let emb: Vec<f64> = serde_json::from_value(p["embedding"].clone()).unwrap();
        assert_eq!(emb.len(), mr_ml::EMBEDDING_DIM);
        // Остался один прототип из 0.5: центроид — его нормаль, а не старое среднее.
        let norm: f64 = emb.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 1e-9);
        assert!((emb[0] - emb[100]).abs() < 1e-12);
    }

    #[test]
    fn remove_last_prototype_empties_profile() {
        let mut p = profile_with(false);
        let (_, empty) = remove_prototype(&mut p, "p1").expect("p1 обязан найтись");
        assert!(empty);
        assert!(remove_prototype(&mut p, "nope").is_none());
    }
}
