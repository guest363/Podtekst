use std::path::Path;

use serde_json::json;

use super::lists::{people_json_files, same_name};
use super::lock_voiceprints;
use super::voiceprints::{parse_profiles, vp_v3};
use crate::config::{app_data_dir, save_dir};
use crate::fsutil::write_atomic;
use crate::http::media::inside_save_dir;
/// Переименовывает профили голоса во всех базах, чтобы привязка не потерялась.
pub(super) fn rename_voiceprint_names(old: &str, new: &str) -> bool {
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

/// Удалённый голос больше не должен показываться как «узнанный» в открытом
/// transcript.json: снимаем matched-флаг, имя кластера оставляем для контекста.
pub(super) fn clear_matched_in_transcript(audio: &Path, name: &str) {
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

pub(super) fn rename_speaker_in_files(
    audio: &Path,
    new_name: &str,
    old_name: Option<&str>,
) -> Option<serde_json::Value> {
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
    // Без исходного имени не знаем, кого переименовывать: молча не трогаем файлы.
    let old = old_name?;
    let source = std::fs::read_to_string(&json_p).ok()?;
    let mut data: serde_json::Value = serde_json::from_str(&source).ok()?;

    // У одной персоны бывает две карточки: профиль и прежний анонимный кластер.
    // Собираем все её имена, иначе переименуется только часть реплик.
    let mut aliases: Vec<String> = Vec::new();
    if let Some(arr) = data
        .get("meta")
        .and_then(|m| m.get("speakers_info"))
        .and_then(|v| v.as_array())
    {
        for spk in arr {
            let cur = spk.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let matched = spk
                .get("matched_name")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if same_name(cur, old) || same_name(matched, old) {
                for value in [cur, matched] {
                    if !value.is_empty() && !aliases.iter().any(|a| a == value) {
                        aliases.push(value.to_string());
                    }
                }
            }
        }
    }
    if !aliases.iter().any(|a| same_name(a, old)) {
        aliases.push(old.to_string());
    }
    if let Some(arr) = data
        .get_mut("meta")
        .and_then(|m| m.get_mut("speakers_info"))
        .and_then(|v| v.as_array_mut())
    {
        for spk in arr.iter_mut() {
            let cur = spk.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let matched = spk
                .get("matched_name")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if aliases
                .iter()
                .any(|a| same_name(cur, a) || same_name(matched, a))
            {
                spk["name"] = json!(new_name);
                spk["matched"] = json!(true);
                spk["matched_name"] = json!(new_name);
            }
        }
    }
    if let Some(list) = data
        .get_mut("meta")
        .and_then(|m| m.get_mut("speakers"))
        .and_then(|v| v.as_array_mut())
    {
        for s in list.iter_mut() {
            if s.as_str()
                .is_some_and(|name| aliases.iter().any(|a| same_name(name, a)))
            {
                *s = json!(new_name);
            }
        }
        let mut deduped: Vec<serde_json::Value> = Vec::new();
        for s in list.drain(..) {
            if !deduped.contains(&s) {
                deduped.push(s);
            }
        }
        *list = deduped;
    }
    if let Some(arr) = data.get_mut("utterances").and_then(|v| v.as_array_mut()) {
        for u in arr.iter_mut() {
            let hit = u
                .get("speaker")
                .and_then(|x| x.as_str())
                .is_some_and(|name| aliases.iter().any(|a| same_name(name, a)));
            if hit {
                u["speaker"] = json!(new_name);
            }
        }
    }
    let _ = write_atomic(
        &json_p,
        serde_json::to_string_pretty(&data)
            .unwrap_or_default()
            .as_bytes(),
    );

    let utterances: Vec<crate::transcribe::Utterance> =
        serde_json::from_value(data["utterances"].clone()).ok()?;
    let markdown = crate::transcribe::report::build_markdown(&data["meta"], &utterances);
    let text = crate::transcribe::report::build_text(&utterances);
    for (path, contents) in [(&md_p, markdown.clone()), (&txt_p, text)] {
        let _ = write_atomic(path, contents.as_bytes());
    }
    let rev = serde_json::from_value::<Vec<mr_summarize::Utterance>>(data["utterances"].clone())
        .ok()
        .map(|items| mr_summarize::artifact::source_rev(&items));
    Some(json!({
        "utterances": data["utterances"],
        "speakers_info": data["meta"]["speakers_info"],
        "markdown": markdown,
        "rev": rev,
    }))
}
