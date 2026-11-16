use std::path::Path;

use mr_packs as packs;
use serde_json::json;

use super::store::{parse_profiles, voiceprint_usable};
use crate::config::app_data_dir;
use crate::http::media::{build_wav_snippet, inside_save_dir};
use crate::voiceprints::EMBEDDING_SPACE;

/// Сниппет нового прототипа: образец нужен, чтобы голос можно было узнать на слух.
pub(super) fn snippet_for_new_prototype(
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

pub(super) fn remove_snippet(pack_dir: &Path, rel: &str) {
    if let Some(path) = packs::safe_snippet_path(pack_dir, rel) {
        let _ = std::fs::remove_file(path);
    }
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
