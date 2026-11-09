use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{
    ensure_default_pack, now_epoch_secs, pack_info_of, packs_root_dir, slugify, validate_pack_id,
    write_json_atomic, PackManifest, PacksError, Result,
};

/// Потолки распаковки считаем по фактически прочитанным байтам: заголовок архива
/// может занижать размер, поэтому `read_to_end` без предела недопустим.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// Защита от архива из миллионов пустых записей.
const MAX_ENTRIES: usize = 4096;

/// Битый JSON — ошибка, а не пустое значение: иначе экспорт молча потеряет данные.
fn read_json_or_empty(path: &Path, empty: Value) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(empty),
        Err(e) => Err(e.into()),
    }
}

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
        let people = read_json_or_empty(&dir.join("people.json"), json!([]))?;
        let voiceprints = read_json_or_empty(
            &dir.join("voiceprints.json"),
            super::voiceprints_v3(vec![], ""),
        )?;
        let bundle = json!({
            "format": "meeting-recorder-voicepack-v2",
            "manifest": manifest,
            "people": people,
            "voiceprints": voiceprints,
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
    import_pack_with_space(app_data, source, merge_into_default, "")
}

/// `embedding_space` — запасное пространство для отпечатков без своего; чужое
/// (импортированное) сохраняем, иначе теряется привязка векторов.
pub fn import_pack_with_space(
    app_data: &Path,
    source: &Path,
    merge_into_default: bool,
    embedding_space: &str,
) -> Result<super::PackInfo> {
    if !source.exists() {
        return Err(PacksError::NotFound(source.to_string_lossy().to_string()));
    }
    ensure_default_pack(app_data)?;
    let is_json = source.extension().is_some_and(|e| e == "json") || looks_like_json(source);

    if !is_json {
        return import_zip(app_data, source, merge_into_default, embedding_space);
    }
    import_json(app_data, source, merge_into_default, embedding_space)
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

#[derive(Clone, Copy)]
enum EntryKind {
    Json(&'static str),
    Snippet,
}

/// Разбор имени записи архива: только одиночный компонент пути.
/// `enclosed_name` отсекает `..` и абсолютные пути, а проверка исходной строки —
/// Windows-разделители и буквы дисков: на Linux они выглядят обычным компонентом
/// и после `Path::join` увели бы запись за пределы пакета.
fn classify_entry<R: Read>(f: &zip::read::ZipFile<'_, R>) -> Option<(EntryKind, String)> {
    let raw = f.name().ok()?;
    if raw.contains('\\') || raw.contains(':') || raw.contains('\0') {
        return None;
    }
    let enclosed = f.enclosed_name()?;
    let mut parts = enclosed.components();
    let first = parts.next()?.as_os_str().to_str()?.to_string();
    let second = parts.next();
    if parts.next().is_some() {
        return None;
    }
    match (first.as_str(), second) {
        ("manifest.json", None) => Some((EntryKind::Json("manifest.json"), first.clone())),
        ("people.json", None) => Some((EntryKind::Json("people.json"), first.clone())),
        ("external_people.json", None) => {
            Some((EntryKind::Json("external_people.json"), first.clone()))
        }
        ("voiceprints.json", None) => Some((EntryKind::Json("voiceprints.json"), first.clone())),
        ("snippets", Some(file)) => {
            let fname = file.as_os_str().to_str()?;
            if !fname.is_empty() && fname.ends_with(".wav") {
                Some((EntryKind::Snippet, fname.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn read_entry_limited<R: Read>(
    f: &mut zip::read::ZipFile<'_, R>,
    consumed: &mut u64,
    max_entry: u64,
    max_total: u64,
) -> Result<Vec<u8>> {
    if f.size() > max_entry {
        return Err(PacksError::Zip(format!(
            "запись {} больше допустимых {max_entry} байт",
            f.name().as_deref().unwrap_or("<unnamed>")
        )));
    }
    let cap = max_entry.min(max_total.saturating_sub(*consumed));
    let mut buf = Vec::new();
    // +1 байт: видно, что запись не влезла, даже если размер в заголовке соврал.
    f.by_ref().take(cap + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(PacksError::Zip(format!(
            "распаковка пакета превышает лимит {max_total} байт"
        )));
    }
    *consumed += buf.len() as u64;
    Ok(buf)
}

/// Первый проход по архиву: манифест/люди/отпечатки + сниппеты целиком в память;
/// повторного открытия архива не нужно.
fn import_zip(
    app_data: &Path,
    source: &Path,
    merge_into_default: bool,
    embedding_space: &str,
) -> Result<super::PackInfo> {
    let file = fs::File::open(source)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| PacksError::Zip(e.to_string()))?;
    if archive.len() > MAX_ENTRIES {
        return Err(PacksError::Zip(format!(
            "слишком много файлов в архиве: {} (максимум {MAX_ENTRIES})",
            archive.len()
        )));
    }
    let mut manifest_val: Option<Value> = None;
    let mut people_val: Option<Value> = None;
    let mut vp_val: Option<Value> = None;
    let mut named: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let mut snippet_blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut consumed = 0u64;
    for i in 0..archive.len() {
        let mut f = archive
            .by_index(i)
            .map_err(|e| PacksError::Zip(e.to_string()))?;
        let Some((kind, fname)) = classify_entry(&f) else {
            continue;
        };
        let buf = read_entry_limited(&mut f, &mut consumed, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES)?;
        match kind {
            EntryKind::Json(tag) => {
                let val: Option<Value> = serde_json::from_slice(&buf).ok();
                match tag {
                    "manifest.json" => manifest_val = val,
                    "people.json" => people_val = val,
                    "voiceprints.json" => vp_val = val,
                    _ => {}
                }
                named.push((tag, buf));
            }
            EntryKind::Snippet => snippet_blobs.push((fname, buf)),
        }
    }
    let people_val = people_val.unwrap_or_else(|| json!([]));
    let vp_val = normalize_voiceprints(
        vp_val.unwrap_or_else(|| json!({"version": 3, "profiles": []})),
        embedding_space,
    );

    if merge_into_default {
        let target = packs_root_dir(app_data).join("default");
        merge_people(&target, &people_val)?;
        merge_voiceprints(&target, &vp_val, embedding_space)?;
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
        // Отпечатки пишем нормализованными ниже, иначе снова потеряем v3.
        if name == "voiceprints.json" {
            continue;
        }
        fs::write(target.join(name), buf)?;
    }
    write_json_atomic(&target.join("voiceprints.json"), &vp_val)?;
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
    embedding_space: &str,
) -> Result<super::PackInfo> {
    let text = fs::read_to_string(source)?;
    let raw: Value = serde_json::from_str(&text)?;
    let people_val = raw.get("people").cloned().unwrap_or_else(|| json!([]));
    let vp_val = normalize_voiceprints(
        raw.get("voiceprints")
            .cloned()
            .unwrap_or_else(|| json!({"version": 3, "profiles": []})),
        embedding_space,
    );
    if merge_into_default {
        let target = packs_root_dir(app_data).join("default");
        merge_people(&target, &people_val)?;
        merge_voiceprints(&target, &vp_val, embedding_space)?;
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

/// Чужое пространство сохраняем: иначе импорт теряет привязку векторов.
fn normalize_voiceprints(vp: Value, embedding_space: &str) -> Value {
    let profiles = match &vp {
        // Массив — legacy/повреждённый формат: не доверяем ему, начинаем с пустого.
        Value::Array(_) => Vec::new(),
        Value::Object(_) => vp
            .get("profiles")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let space = vp
        .get("embedding_space")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(embedding_space);
    super::voiceprints_v3(profiles, space)
}

fn profile_key(profile: &Value) -> Option<String> {
    let name = profile.get("name")?.as_str()?.trim().to_lowercase();
    (!name.is_empty()).then_some(name)
}

/// Существующий профиль важнее импортированного: импорт не должен затирать
/// накопленную биометрию.
fn merge_voiceprints(target_dir: &Path, new_vp: &Value, embedding_space: &str) -> Result<()> {
    let vp_file = target_dir.join("voiceprints.json");
    let existing: Value = match fs::read_to_string(&vp_file) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            super::voiceprints_v3(vec![], embedding_space)
        }
        Err(e) => return Err(e.into()),
    };
    let existing = normalize_voiceprints(existing, embedding_space);
    let incoming = normalize_voiceprints(new_vp.clone(), embedding_space);
    let space = existing
        .get("embedding_space")
        .and_then(|s| s.as_str())
        .or_else(|| incoming.get("embedding_space").and_then(|s| s.as_str()))
        .unwrap_or(embedding_space);
    let mut profiles: Vec<Value> = existing
        .get("profiles")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut seen: HashSet<String> = profiles.iter().filter_map(profile_key).collect();
    if let Some(items) = incoming.get("profiles").and_then(|p| p.as_array()) {
        for profile in items {
            let Some(key) = profile_key(profile) else {
                continue;
            };
            if seen.insert(key) {
                profiles.push(profile.clone());
            }
        }
    }
    write_json_atomic(&vp_file, &super::voiceprints_v3(profiles, space))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tmp_app;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, data).unwrap();
        }
        writer.finish().unwrap();
    }

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
        merge_voiceprints(&target, &new_vp, "space-a").unwrap();
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(target.join("voiceprints.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["version"], 3);
        assert_eq!(v["embedding_space"], "space-a");
        assert_eq!(v["profiles"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn merge_voiceprints_keeps_existing_profile_on_collision() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("vp-merge");
        let target = crate::packs_root_dir(&app).join("default");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join("voiceprints.json"),
            r#"{"version": 3, "embedding_space": "s", "profiles": [{"name": "Профиль A", "embedding": [1.0]}]}"#,
        )
        .unwrap();
        let new_vp = json!({
            "version": 3,
            "embedding_space": "s",
            "profiles": [
                {"name": "профиль a", "embedding": [2.0]},
                {"name": "Профиль B", "embedding": [3.0]},
            ],
        });
        merge_voiceprints(&target, &new_vp, "s").unwrap();
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(target.join("voiceprints.json")).unwrap(),
        )
        .unwrap();
        let profiles = v["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
        let kept = profiles.iter().find(|p| p["name"] == "Профиль A").unwrap();
        assert_eq!(kept["embedding"][0].as_f64(), Some(1.0));
    }

    #[test]
    fn merge_voiceprints_errors_on_corrupted_existing() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("corrupt-vp");
        let target = crate::packs_root_dir(&app).join("default");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("voiceprints.json"), "не json").unwrap();
        assert!(merge_voiceprints(&target, &json!({}), "s").is_err());
        assert_eq!(
            std::fs::read_to_string(target.join("voiceprints.json")).unwrap(),
            "не json"
        );
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

    #[test]
    fn export_errors_on_corrupted_people_json() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("export-corrupt");
        crate::ensure_default_pack(&app).unwrap();
        let dir = crate::packs_root_dir(&app).join("default");
        std::fs::write(dir.join("people.json"), "не json").unwrap();
        let out = app.join("bad.voicepack.json");
        assert!(export_pack(&app, "default", &out, false).is_err());
    }

    #[test]
    fn zip_slip_entries_never_escape_target() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("zip-slip");
        let src = app.join("evil.voicepack.zip");
        write_zip(
            &src,
            &[
                ("snippets/../../escaped.wav", &b"RIFF"[..]),
                ("snippets/C:\\Users\\Public\\escaped.wav", &b"RIFF"[..]),
                ("../escaped.wav", &b"RIFF"[..]),
            ],
        );
        let imported = import_pack(&app, &src, false).unwrap();
        for path in [
            app.join("escaped.wav"),
            app.join("packs").join("escaped.wav"),
            app.parent().unwrap().join("escaped.wav"),
        ] {
            assert!(!path.exists(), "архив записал {path:?}");
        }
        let snippets = crate::packs_root_dir(&app)
            .join(&imported.id)
            .join("snippets");
        assert_eq!(std::fs::read_dir(snippets).unwrap().count(), 0);
    }

    #[test]
    fn oversized_zip_entry_is_rejected() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("zip-bomb");
        let src = app.join("bomb.zip");
        let zeros = [0u8; 32];
        write_zip(&src, &[("snippets/a.wav", &zeros[..])]);

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&src).unwrap()).unwrap();
        let mut entry = archive.by_index(0).unwrap();
        let mut consumed = 0;
        assert!(read_entry_limited(&mut entry, &mut consumed, 16, 1024).is_err());

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&src).unwrap()).unwrap();
        let mut entry = archive.by_index(0).unwrap();
        let mut consumed = 0;
        assert!(read_entry_limited(&mut entry, &mut consumed, 1024, 24).is_err());
    }

    #[test]
    fn import_preserves_incoming_embedding_space() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("import-space");
        let src = app.join("pack.json");
        std::fs::write(
            &src,
            r#"{"manifest":{"id":"x"},"people":[],"voiceprints":{"version":2,"embedding_space":"space-a","profiles":[{"name":"A"}]}}"#,
        )
        .unwrap();
        let pack = import_pack(&app, &src, false).unwrap();
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(
                crate::packs_root_dir(&app)
                    .join(&pack.id)
                    .join("voiceprints.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(v["version"], 3);
        assert_eq!(v["embedding_space"], "space-a");
        assert_eq!(v["profiles"].as_array().unwrap().len(), 1);

        let src2 = app.join("pack2.json");
        std::fs::write(&src2, r#"{"voiceprints":{"version":2,"profiles":[]}}"#).unwrap();
        let pack2 = import_pack_with_space(&app, &src2, false, "space-b").unwrap();
        let v2: Value = serde_json::from_str(
            &std::fs::read_to_string(
                crate::packs_root_dir(&app)
                    .join(&pack2.id)
                    .join("voiceprints.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(v2["version"], 3);
        assert_eq!(v2["embedding_space"], "space-b");
    }
}
