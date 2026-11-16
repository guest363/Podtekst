use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use mr_packs as packs;
use serde_json::json;

use super::lists::{ensure_person_in_pack, resolve_target_pack, same_name};
use super::lock_voiceprints;
use super::transcript_names::{clear_matched_in_transcript, rename_speaker_in_files};
use crate::config::{app_data_dir, save_dir};
use crate::fsutil::write_atomic;
use crate::http::media::{build_wav_snippet, inside_save_dir, resolve_media_path};
use crate::http::{json_response, query_param};
use crate::voiceprints::{
    cosine_similarity, l2_normalize, weighted_centroid, EMBEDDING_DIM, EMBEDDING_SPACE,
    MAX_PROTOTYPES,
};

static PROTO_SEQ: AtomicUsize = AtomicUsize::new(0);
/// Профили из voiceprints.json любой схемы: массив (legacy) или v2/v3 `{"profiles": []}`.
pub(super) fn parse_profiles(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.get("profiles")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_else(|| v.as_array().cloned().unwrap_or_default())
}

pub(super) fn vp_v3(profiles: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"version": 3, "embedding_space": EMBEDDING_SPACE, "profiles": profiles})
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

pub(crate) fn load_all_voiceprints() -> Vec<serde_json::Value> {
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

fn prototype_duration(start: Option<f64>, end: Option<f64>) -> f64 {
    match (start, end) {
        (Some(s), Some(e)) if e > s => (e - s).clamp(0.4, 15.0),
        _ => 4.0,
    }
}

pub(super) fn prototypes_centroid(protos: &[serde_json::Value]) -> (Vec<f64>, usize) {
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
pub(super) struct PrototypeSample<'a> {
    pub(super) audio_file: &'a str,
    pub(super) start: Option<f64>,
    pub(super) end: Option<f64>,
    pub(super) channel: Option<usize>,
    pub(super) quality: Option<f64>,
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

pub(super) fn store_voiceprint(
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

pub(crate) fn handle_voiceprint_save(request: tiny_http::Request, body: serde_json::Value) {
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
    let mut state = None;
    if !audio_file.is_empty() {
        // Правим расшифровки только внутри папки записей (запись в произвольную
        // директорию через чужое имя файла недопустима). Относительное имя
        // сначала разрешаем от `save_dir`, иначе файл не найдётся.
        let audio_path = resolve_media_path(&audio_file);
        if inside_save_dir(&audio_path) {
            state = rename_speaker_in_files(
                &audio_path,
                &name,
                if old_name.is_empty() {
                    None
                } else {
                    Some(old_name.as_str())
                },
            );
        }
    }
    // UI обновляет текст из этого ответа, без повторного GET: иначе на медленной
    // цепочке (loadPeople → reload) расшифровка оставалась старой.
    let mut payload =
        json!({"ok": true, "voiceprint_saved": !emb.is_empty(), "pack_id": chosen_pack});
    if let (Some(dst), Some(src)) = (
        payload.as_object_mut(),
        state.and_then(|v| v.as_object().cloned()),
    ) {
        for (key, value) in src {
            dst.insert(key, value);
        }
    }
    json_response(request, payload, 200);
}

#[cfg(not(feature = "onnx"))]
pub(crate) fn handle_voiceprint_learn_sample(
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
pub(crate) fn handle_voiceprint_learn_sample(request: tiny_http::Request, body: serde_json::Value) {
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

pub(crate) fn handle_voiceprint_delete(request: tiny_http::Request, body: serde_json::Value) {
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
    let bytes = build_wav_snippet(audio, s, e, channel)?;
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
    let found = delete_voiceprint_files(name, voiceprint_files());
    if found {
        if let Some(af) = audio_file.filter(|s| !s.is_empty()) {
            clear_matched_in_transcript(Path::new(af), name.trim());
        }
    }
    found
}

pub(super) fn pack_voiceprint_file(pack_id: &str) -> std::path::PathBuf {
    packs::packs_root_dir(&app_data_dir())
        .join(pack_id)
        .join("voiceprints.json")
}

pub(super) fn delete_voiceprint_files(name: &str, files: Vec<std::path::PathBuf>) -> bool {
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
pub(super) fn remove_prototype(
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
