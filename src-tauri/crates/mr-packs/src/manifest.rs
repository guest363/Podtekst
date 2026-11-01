use std::fs;
use std::path::Path;

use serde_json::json;

use super::{
    now_epoch_secs, packs_root_dir, validate_pack_id, write_json_atomic, PackInfo, PackManifest,
    PacksError, Result,
};

/// Прочитать манифест пакета; битый JSON — это ошибка (данные не подменяем суррогатом).
pub(crate) fn read_manifest(pack_dir: &Path) -> Result<PackManifest> {
    let path = pack_dir.join("manifest.json");
    let dirname = || {
        pack_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    match fs::read_to_string(&path) {
        Ok(text) => {
            let mut m: PackManifest = serde_json::from_str(&text)?;
            if m.id.is_empty() {
                m.id = dirname();
            }
            Ok(m)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PackManifest {
            id: dirname(),
            name: dirname(),
            version: 2,
            enabled: true,
            description: String::new(),
            created_at: now_epoch_secs(),
        }),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn count_json_array(path: &Path, key: Option<&str>) -> usize {
    let Ok(text) = fs::read_to_string(path) else {
        return 0;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    match (&v, key) {
        (serde_json::Value::Array(a), _) => a.len(),
        (serde_json::Value::Object(map), Some(k)) => map
            .get(k)
            .and_then(|x| x.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
        _ => 0,
    }
}

pub fn ensure_default_pack(app_data: &Path) -> Result<std::path::PathBuf> {
    let dir = packs_root_dir(app_data).join("default");
    fs::create_dir_all(dir.join("snippets"))?;
    if !dir.join("manifest.json").exists() {
        let m = PackManifest {
            id: "default".to_string(),
            name: "Основной профиль".to_string(),
            version: 2,
            enabled: true,
            description: "Локальная база участников и голосовых отпечатков".to_string(),
            created_at: now_epoch_secs(),
        };
        write_json_atomic(&dir.join("manifest.json"), &m)?;
    }
    Ok(dir)
}

pub fn list_packs(app_data: &Path) -> Result<Vec<PackInfo>> {
    ensure_default_pack(app_data)?;
    let root = packs_root_dir(app_data);
    let mut out = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&root)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        (if n == "default" { 0 } else { 1 }, n)
    });
    for entry in entries {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let m = read_manifest(&dir)?;
        let people_count = count_json_array(&dir.join("people.json"), None);
        let voiceprints_count = count_json_array(&dir.join("voiceprints.json"), Some("profiles"));
        let snippets_count = fs::read_dir(dir.join("snippets"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "wav"))
                    .count()
            })
            .unwrap_or(0);
        out.push(PackInfo {
            id: m.id,
            name: m.name,
            version: m.version,
            enabled: m.enabled,
            description: m.description,
            created_at: m.created_at,
            path: dir,
            people_count,
            voiceprints_count,
            snippets_count,
        });
    }
    Ok(out)
}

/// Директории включённых пакетов — лёгкий путь без парсинга people/voiceprints
/// (вызывается на каждой транскрибации и в каждом people-эндпоинте).
pub fn get_active_packs(app_data: &Path) -> Result<Vec<std::path::PathBuf>> {
    ensure_default_pack(app_data)?;
    let root = packs_root_dir(app_data);
    let mut out = Vec::new();
    for entry in fs::read_dir(&root)?.filter_map(|e| e.ok()) {
        let dir = entry.path();
        if dir.is_dir() && read_manifest(&dir).map(|m| m.enabled).unwrap_or(false) {
            out.push(dir);
        }
    }
    Ok(out)
}

/// Включить/выключить пакет. Битый манифест — ошибка, а не тихая перезапись
/// (иначе одно нажатие чекбокса стёрло бы имя и описание пакета).
pub fn set_pack_enabled(app_data: &Path, pack_id: &str, enabled: bool) -> Result<bool> {
    let pack_id = validate_pack_id(pack_id)?;
    let dir = packs_root_dir(app_data).join(pack_id);
    let manifest_path = dir.join("manifest.json");
    if !dir.is_dir() {
        return Ok(false);
    }
    // Патчим только поле enabled в существующем JSON — чужие поля сохраняются.
    let mut m = read_manifest(&dir)?;
    m.enabled = enabled;
    write_json_atomic(&manifest_path, &m)?;
    Ok(true)
}

pub fn create_pack(app_data: &Path, name: &str, description: &str) -> Result<PackInfo> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err(PacksError::Other(
            "Название пакета не может быть пустым".to_string(),
        ));
    }
    let root = packs_root_dir(app_data);
    let mut pid = super::slugify(clean);
    let base = pid.clone();
    let mut idx = 2;
    while root.join(&pid).exists() {
        pid = format!("{base}-{idx}");
        idx += 1;
    }
    let dir = root.join(&pid);
    fs::create_dir_all(dir.join("snippets"))?;
    let m = PackManifest {
        id: pid.clone(),
        name: clean.to_string(),
        version: 2,
        enabled: true,
        description: description.trim().to_string(),
        created_at: now_epoch_secs(),
    };
    write_json_atomic(&dir.join("manifest.json"), &m)?;
    write_json_atomic(&dir.join("people.json"), &json!([]))?;
    write_json_atomic(
        &dir.join("voiceprints.json"),
        &json!({"version": 2, "profiles": []}),
    )?;
    Ok(PackInfo {
        id: m.id,
        name: m.name,
        version: m.version,
        enabled: m.enabled,
        description: m.description,
        created_at: m.created_at,
        path: dir,
        people_count: 0,
        voiceprints_count: 0,
        snippets_count: 0,
    })
}

pub fn delete_pack(app_data: &Path, pack_id: &str) -> Result<bool> {
    let pack_id = validate_pack_id(pack_id)?;
    if pack_id == "default" {
        return Ok(false);
    }
    let dir = packs_root_dir(app_data).join(pack_id);
    if !dir.is_dir() {
        return Ok(false);
    }
    fs::remove_dir_all(&dir)?;
    Ok(!dir.exists())
}

pub fn pack_info_of(app_data: &Path, pack_id: &str) -> Result<PackInfo> {
    let pack_id = validate_pack_id(pack_id)?;
    let infos = list_packs(app_data)?;
    infos
        .into_iter()
        .find(|p| p.id == pack_id)
        .ok_or_else(|| PacksError::NotFound(pack_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tmp_app;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn clean_startup_creates_default() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("default");
        let packs = list_packs(&app).unwrap();
        assert!(packs.iter().any(|p| p.id == "default"));
    }

    #[test]
    fn toggle_and_delete() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("toggle");
        let pack = create_pack(&app, "Временный", "").unwrap();
        assert!(set_pack_enabled(&app, &pack.id, false).unwrap());
        let infos = list_packs(&app).unwrap();
        let found = infos.iter().find(|p| p.id == pack.id).unwrap();
        assert!(!found.enabled);
        assert!(delete_pack(&app, &pack.id).unwrap());
        assert!(!delete_pack(&app, "default").unwrap());
    }

    #[test]
    fn traversal_rejected() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("traversal");
        for bad in ["../evil", "a/b", "a\\b", "..", ""] {
            assert!(set_pack_enabled(&app, bad, true).is_err(), "{bad:?}");
            assert!(delete_pack(&app, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn corrupted_manifest_is_error_not_silent_reset() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("corrupt");
        let pack = create_pack(&app, "Имя пакета", "описание").unwrap();
        let dir = packs_root_dir(&app).join(&pack.id);
        std::fs::write(dir.join("manifest.json"), "{ битый json").unwrap();
        let err = set_pack_enabled(&app, &pack.id, false).unwrap_err();
        assert!(err.to_string().contains("JSON"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.join("manifest.json")).unwrap(),
            "{ битый json"
        );
    }

    #[test]
    fn active_packs_light_path_skips_disabled() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_g, app) = tmp_app("active");
        let off = create_pack(&app, "Выключенный", "").unwrap();
        assert!(set_pack_enabled(&app, &off.id, false).unwrap());
        let active = get_active_packs(&app).unwrap();
        assert!(active.iter().any(|p| p.ends_with("default")));
        assert_eq!(active.len(), 1);
    }
}
