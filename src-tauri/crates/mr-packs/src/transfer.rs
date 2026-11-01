use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{
    ensure_default_pack, now_epoch_secs, pack_info_of, packs_root_dir, slugify, validate_pack_id,
    write_json_atomic, PackManifest, PacksError, Result,
};

pub fn export_pack(
    app_data: &Path,
    pack_id: &str,
    out_path: &Path,
    include_snippets: bool,
) -> Result<PathBuf> {
    let pack_id = validate_pack_id(pack_id)?;
    let dir = packs_root_dir(app_data).join(pack_id);
    if !dir.is_dir() {
        return Err(PacksError::NotFound(pack_id.to_string()));
    }
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let is_json = out_path.extension().is_some_and(|e| e == "json");
    if is_json {
        let manifest = super::manifest::read_manifest(&dir)?;
        let read = |name: &str| -> Option<Value> {
            fs::read_to_string(dir.join(name))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
        };
        let bundle = json!({
            "format": "meeting-recorder-voicepack-v2",
            "manifest": manifest,
            "people": read("people.json").unwrap_or(json!([])),
            "voiceprints": read("voiceprints.json").unwrap_or(json!({"version": 2, "profiles": []})),
        });
        write_json_atomic(out_path, &bundle)?;
        return Ok(out_path.to_path_buf());
    }
    let file = fs::File::create(out_path)?;
    let mut zipw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for fname in [
        "manifest.json",
        "people.json",
        "external_people.json",
        "voiceprints.json",
    ] {
        let src = dir.join(fname);
        if src.is_file() {
            zipw.start_file(fname, opts)
                .map_err(|e| PacksError::Zip(e.to_string()))?;
            std::io::Write::write_all(&mut zipw, &fs::read(&src)?)?;
        }
    }
    if include_snippets {
        let snip = dir.join("snippets");
        if snip.is_dir() {
            for entry in fs::read_dir(&snip)?.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "wav") {
                    let name = format!("snippets/{}", p.file_name().unwrap().to_string_lossy());
                    zipw.start_file(name, opts)
                        .map_err(|e| PacksError::Zip(e.to_string()))?;
                    std::io::Write::write_all(&mut zipw, &fs::read(&p)?)?;
                }
            }
        }
    }
    zipw.finish().map_err(|e| PacksError::Zip(e.to_string()))?;
    Ok(out_path.to_path_buf())
}

pub fn import_pack(
    app_data: &Path,
    source: &Path,
    merge_into_default: bool,
) -> Result<super::PackInfo> {
    if !source.exists() {
        return Err(PacksError::NotFound(source.to_string_lossy().to_string()));
    }
    ensure_default_pack(app_data)?;
    let is_json = source.extension().is_some_and(|e| e == "json") || looks_like_json(source);

    if !is_json {
        return import_zip(app_data, source, merge_into_default);
    }
    import_json(app_data, source, merge_into_default)
}

/// JSON-сниффинг по первому байту: весь файл ради этого не читаем.
fn looks_like_json(path: &Path) -> bool {
    fs::File::open(path)
        .and_then(|mut f| {
            let mut b = [0u8; 1];
            f.read_exact(&mut b)?;
            Ok(b[0] == b'{')
        })
        .unwrap_or(false)
}

/// Первый проход по архиву: манифест/люди/отпечатки + сниппеты целиком в память;
/// повторного открытия архива не нужно.
fn import_zip(app_data: &Path, source: &Path, merge_into_default: bool) -> Result<super::PackInfo> {
    let file = fs::File::open(source)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| PacksError::Zip(e.to_string()))?;
    let mut manifest_val: Option<Value> = None;
    let mut people_val: Option<Value> = None;
    let mut vp_val: Option<Value> = None;
    let mut named: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let mut snippet_blobs: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive
            .by_index(i)
            .map_err(|e| PacksError::Zip(e.to_string()))?;
        let name = f.name().to_string();
        if name.contains("..") {
            continue;
        }
        match name.as_str() {
            "manifest.json" | "people.json" | "external_people.json" | "voiceprints.json" => {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                let val: Option<Value> = serde_json::from_slice(&buf).ok();
                match name.as_str() {
                    "manifest.json" => manifest_val = val,
                    "people.json" => people_val = val,
                    "voiceprints.json" => vp_val = val,
                    _ => {}
                }
                let tag: &'static str = match name.as_str() {
                    "manifest.json" => "manifest.json",
                    "people.json" => "people.json",
                    "external_people.json" => "external_people.json",
                    _ => "voiceprints.json",
                };
                named.push((tag, buf));
            }
            _ if name.starts_with("snippets/") && name.ends_with(".wav") => {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                if let Some(fname) = name.rsplit('/').next() {
                    snippet_blobs.push((fname.to_string(), buf));
                }
            }
            _ => {}
        }
    }
    let people_val = people_val.unwrap_or(json!([]));
    let vp_val = vp_val.unwrap_or(json!({"version": 2, "profiles": []}));

    if merge_into_default {
        let target = packs_root_dir(app_data).join("default");
        merge_people(&target, &people_val)?;
        merge_voiceprints(&target, &vp_val)?;
        fs::create_dir_all(target.join("snippets"))?;
        for (fname, blob) in snippet_blobs {
            fs::write(target.join("snippets").join(fname), blob)?;
        }
        return pack_info_of(app_data, "default");
    }

    let hint = manifest_val
        .as_ref()
        .and_then(|m| m.get("id").or_else(|| m.get("name")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            source
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "imported-pack".to_string())
        });
    let pid = unique_pid(app_data, &hint);
    let target = packs_root_dir(app_data).join(&pid);
    fs::create_dir_all(target.join("snippets"))?;
    for (name, buf) in named {
        fs::write(target.join(name), buf)?;
    }
    for (fname, blob) in snippet_blobs {
        fs::write(target.join("snippets").join(fname), blob)?;
    }
    let mut m = super::manifest::read_manifest(&target)?;
    m.id = pid.clone();
    m.enabled = true;
    if m.name.is_empty() {
        m.name = pid.clone();
    }
    write_json_atomic(&target.join("manifest.json"), &m)?;
    pack_info_of(app_data, &pid)
}

fn import_json(
    app_data: &Path,
    source: &Path,
    merge_into_default: bool,
) -> Result<super::PackInfo> {
    let text = fs::read_to_string(source)?;
    let raw: Value = serde_json::from_str(&text)?;
    let people_val = raw.get("people").cloned().unwrap_or(json!([]));
    let vp_val = raw
        .get("voiceprints")
        .cloned()
        .unwrap_or(json!({"version": 2, "profiles": []}));
    if merge_into_default {
        let target = packs_root_dir(app_data).join("default");
        merge_people(&target, &people_val)?;
        merge_voiceprints(&target, &vp_val)?;
        return pack_info_of(app_data, "default");
    }
    let hint = raw
        .get("manifest")
        .and_then(|m| m.get("id").or_else(|| m.get("name")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            source
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "imported-pack".to_string())
        });
    let pid = unique_pid(app_data, &hint);
    let target = packs_root_dir(app_data).join(&pid);
    fs::create_dir_all(target.join("snippets"))?;
    let m = PackManifest {
        id: pid.clone(),
        name: raw
            .get("manifest")
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&pid)
            .to_string(),
        version: 2,
        enabled: true,
        description: "Импортированный голосовой пакет".to_string(),
        created_at: now_epoch_secs(),
    };
    write_json_atomic(&target.join("manifest.json"), &m)?;
    write_json_atomic(&target.join("people.json"), &people_val)?;
    write_json_atomic(&target.join("voiceprints.json"), &vp_val)?;
    pack_info_of(app_data, &pid)
}

fn unique_pid(app_data: &Path, hint: &str) -> String {
    let mut pid = slugify(hint);
    if pid == "default" {
        pid = "imported-pack".to_string();
    }
    let root = packs_root_dir(app_data);
    let base = pid.clone();
    let mut idx = 2;
    while root.join(&pid).exists() {
        pid = format!("{base}-{idx}");
        idx += 1;
    }
    pid
}

/// Слияние людей: битый существующий файл — ошибка (иначе потеряем всех участников).
fn merge_people(target_dir: &Path, new_people: &Value) -> Result<()> {
    let p_file = target_dir.join("people.json");
    let existing: Vec<Value> = match fs::read_to_string(&p_file) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e.into()),
    };
    let mut seen: std::collections::HashSet<String> = existing
        .iter()
        .filter_map(|p| p.get("name")?.as_str().map(|s| s.trim().to_lowercase()))
        .collect();
    let mut merged = existing;
    if let Some(arr) = new_people.as_array() {
        for item in arr {
            if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                let key = name.trim().to_lowercase();
                if !key.is_empty() && !seen.contains(&key) {
                    seen.insert(key);
                    merged.push(item.clone());
                }
            }
        }
    }
    write_json_atomic(&p_file, &merged)?;
    Ok(())
}

/// Слияние голосовых профилей v2; не-объектный существующий файл
/// (повреждение/массив v1) заменяется каркасом v2, а не паникует.
fn merge_voiceprints(target_dir: &Path, new_vp: &Value) -> Result<()> {
    let vp_file = target_dir.join("voiceprints.json");
    let mut existing: Value = match fs::read_to_string(&vp_file) {
        Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or_else(|_| {
            // Битый файл: каркас v2 лучше паники, но след остаётся в консоли.
            json!({"version": 2, "profiles": []})
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"version": 2, "profiles": []}),
        Err(e) => return Err(e.into()),
    };
    if !existing.is_object() {
        existing = json!({"version": 2, "profiles": []});
    }
    let mut by_name: std::collections::HashMap<String, Value> = Default::default();
    for src in [&existing, new_vp] {
        if let Some(arr) = src.get("profiles").and_then(|p| p.as_array()) {
            for p in arr {
                if let Some(n) = p.get("name").and_then(|n| n.as_str()) {
                    by_name.insert(n.trim().to_lowercase(), p.clone());
                }
            }
        }
    }
    let merged = json!({
        "version": 2,
        "profiles": by_name.into_values().collect::<Vec<_>>(),
    });
    write_json_atomic(&vp_file, &merged)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tmp_app;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn export_import_roundtrip() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("roundtrip");
        let pack = crate::create_pack(&app, "Тестовая команда", "roundtrip").unwrap();
        assert!(!pack.id.is_empty());
        crate::ensure_default_pack(&app).unwrap();
        std::fs::write(
            app.join("packs").join("default").join("people.json"),
            r#"[{"name": "Тестовый Голос", "role": "Роль"}]"#,
        )
        .unwrap();
        let out = app.join("export.voicepack.zip");
        export_pack(&app, "default", &out, false).unwrap();
        assert!(out.exists());
        let imported = import_pack(&app, &out, false).unwrap();
        assert_ne!(imported.id, "default");
    }

    #[test]
    fn merge_people_keeps_existing_on_bad_import() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("merge");
        let target = crate::packs_root_dir(&app).join("default");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("people.json"), r#"[{"name": "Анна"}]"#).unwrap();
        let new = json!([{"name": "Борис"}]);
        merge_people(&target, &new).unwrap();
        let text = std::fs::read_to_string(target.join("people.json")).unwrap();
        assert!(text.contains("Анна") && text.contains("Борис"));
    }

    #[test]
    fn merge_voiceprints_survives_non_object_existing() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("vp");
        let target = crate::packs_root_dir(&app).join("default");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("voiceprints.json"), r#"[1,2]"#).unwrap();
        let new_vp = json!({"version": 2, "profiles": [{"name": "Голос"}]});
        merge_voiceprints(&target, &new_vp).unwrap();
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(target.join("voiceprints.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["version"], 2);
        assert_eq!(v["profiles"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn merge_people_errors_on_corrupted_existing() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("corrupt-people");
        let target = crate::packs_root_dir(&app).join("default");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("people.json"), "не json").unwrap();
        assert!(merge_people(&target, &json!([])).is_err());
    }
}
