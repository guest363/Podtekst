use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::{
    packs_root_dir, safe_snippet_rel, validate_pack_id, voiceprints_v3, write_json_atomic,
    PacksError, Result,
};

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

pub fn add_person_to_pack(app_data: &Path, pack_id: &str, mut person: Value) -> Result<()> {
    let pack_id = validate_pack_id(pack_id)?;
    let dir = packs_root_dir(app_data).join(pack_id);
    if !dir.is_dir() {
        return Err(PacksError::NotFound(pack_id.to_string()));
    }
    let name = person
        .get("name")
        .and_then(|x| x.as_str())
        .map(|s| s.trim())
        .unwrap_or("");
    if name.is_empty() {
        return Err(PacksError::Other("Укажите имя участника".to_string()));
    }
    let file = dir.join("people.json");
    let mut list: Vec<Value> = match fs::read_to_string(&file) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    let mut found = false;
    for p in &mut list {
        if same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), name) {
            if let Some(role) = person.get("role") {
                p["role"] = role.clone();
            }
            found = true;
            break;
        }
    }
    if !found {
        if person.get("role").is_none() {
            person["role"] = json!("Участник");
        }
        list.push(person);
    }
    write_json_atomic(&file, &list)
}

pub fn remove_person_from_pack(app_data: &Path, pack_id: &str, name: &str) -> Result<bool> {
    let pack_id = validate_pack_id(pack_id)?;
    let dir = packs_root_dir(app_data).join(pack_id);
    if !dir.is_dir() {
        return Ok(false);
    }
    let file = dir.join("people.json");
    let Ok(text) = fs::read_to_string(&file) else {
        return Ok(false);
    };
    let Ok(list) = serde_json::from_str::<Vec<Value>>(&text) else {
        return Ok(false);
    };
    let before = list.len();
    let kept: Vec<Value> = list
        .into_iter()
        .filter(|p| !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
        .collect();
    if kept.len() != before {
        write_json_atomic(&file, &kept)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn move_person_pack(
    app_data: &Path,
    name: &str,
    from_pack_id: &str,
    to_pack_id: &str,
) -> Result<()> {
    let from_id = validate_pack_id(from_pack_id)?;
    let to_id = validate_pack_id(to_pack_id)?;
    if from_id == to_id {
        return Ok(());
    }
    let from_dir = packs_root_dir(app_data).join(from_id);
    let to_dir = packs_root_dir(app_data).join(to_id);
    if !from_dir.is_dir() {
        return Err(PacksError::NotFound(from_id.to_string()));
    }
    if !to_dir.is_dir() {
        return Err(PacksError::NotFound(to_id.to_string()));
    }

    // 1. Извлекаем и удаляем человека из from_dir/people.json
    let from_people_file = from_dir.join("people.json");
    let from_people: Vec<Value> = match fs::read_to_string(&from_people_file) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    let mut moved_person = None;
    let kept_people: Vec<Value> = from_people
        .into_iter()
        .filter(|p| {
            if same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), name) {
                moved_person = Some(p.clone());
                false
            } else {
                true
            }
        })
        .collect();
    write_json_atomic(&from_people_file, &kept_people)?;

    // 2. Извлекаем профиль отпечатка из from_dir/voiceprints.json (или fallback из default)
    let mut source_vp_dir = from_dir.clone();
    let mut moved_profile = None;
    let extract_vp = |vp_dir: &Path| -> Option<Value> {
        let vp_file = vp_dir.join("voiceprints.json");
        let text = fs::read_to_string(&vp_file).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let space = v
            .get("embedding_space")
            .and_then(|s| s.as_str())
            .unwrap_or("eres2netv2-192d")
            .to_string();
        let profiles = v
            .get("profiles")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let mut found = None;
        let kept_profiles: Vec<Value> = profiles
            .into_iter()
            .filter(|p| {
                if same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), name) {
                    found = Some(p.clone());
                    false
                } else {
                    true
                }
            })
            .collect();
        if found.is_some() {
            let _ = write_json_atomic(&vp_file, &voiceprints_v3(kept_profiles, &space));
        }
        found
    };

    if let Some(prof) = extract_vp(&from_dir) {
        moved_profile = Some(prof);
    } else if from_id != "default" {
        let def_dir = packs_root_dir(app_data).join("default");
        if def_dir.is_dir() {
            if let Some(prof) = extract_vp(&def_dir) {
                moved_profile = Some(prof);
                source_vp_dir = def_dir;
            }
        }
    }

    // 3. Переносим аудиосниппеты
    if let Some(ref prof) = moved_profile {
        if let Some(protos) = prof.get("prototypes").and_then(|x| x.as_array()) {
            for proto in protos {
                if let Some(rel) = proto.get("snippet_relpath").and_then(|x| x.as_str()) {
                    let Some(rel) = safe_snippet_rel(rel) else {
                        continue;
                    };
                    let from_wav = source_vp_dir.join(rel);
                    let to_wav = to_dir.join(rel);
                    if !from_wav.is_file() {
                        continue;
                    }
                    let Some(root) = source_vp_dir.canonicalize().ok() else {
                        continue;
                    };
                    let Some(real) = from_wav.canonicalize().ok() else {
                        continue;
                    };
                    if !real.starts_with(&root) {
                        continue;
                    }
                    if let Some(p) = to_wav.parent() {
                        let _ = fs::create_dir_all(p);
                    }
                    let _ = fs::rename(&real, &to_wav)
                        .or_else(|_| fs::copy(&real, &to_wav).map(|_| ()));
                    let _ = fs::remove_file(&real);
                }
            }
        }
    }

    // 4. Добавляем человека в to_dir/people.json
    let person_val = moved_person.unwrap_or_else(|| json!({"name": name, "role": "Участник"}));
    add_person_to_pack(app_data, to_id, person_val)?;

    // 5. Добавляем профиль в to_dir/voiceprints.json
    if let Some(prof) = moved_profile {
        let to_vp_file = to_dir.join("voiceprints.json");
        let mut to_profiles = vec![];
        let mut to_space = "eres2netv2-192d".to_string();
        match fs::read_to_string(&to_vp_file) {
            Ok(text) => {
                let v: Value = serde_json::from_str(&text)?;
                if let Some(s) = v.get("embedding_space").and_then(|x| x.as_str()) {
                    to_space = s.to_string();
                }
                if let Some(arr) = v.get("profiles").and_then(|x| x.as_array()) {
                    to_profiles = arr.clone();
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        to_profiles
            .retain(|p| !same_name(p.get("name").and_then(|x| x.as_str()).unwrap_or(""), name));
        to_profiles.push(prof);
        write_json_atomic(&to_vp_file, &voiceprints_v3(to_profiles, &to_space))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::create_pack;
    use crate::testutil::tmp_app;

    #[test]
    fn move_keeps_snippets_inside_pack() {
        let (_g, app) = tmp_app("move-snippet-traversal");
        let p1 = create_pack(&app, "Отдел 1", "").unwrap();
        let p2 = create_pack(&app, "Отдел 2", "").unwrap();
        add_person_to_pack(&app, &p1.id, json!({"name": "Инженер 2"})).unwrap();

        let absolute = app.join("absolute.txt");
        let relative = app.join("packs").join("relative.txt");
        fs::write(&absolute, b"keep").unwrap();
        fs::write(&relative, b"keep").unwrap();
        let vp = voiceprints_v3(
            vec![json!({
                "name": "Инженер 2",
                "prototypes": [
                    {"id": "p1", "embedding": [], "snippet_relpath": absolute.to_string_lossy()},
                    {"id": "p2", "embedding": [], "snippet_relpath": "../relative.txt"}
                ]
            })],
            "eres2netv2-192d",
        );
        fs::write(
            app.join("packs").join(&p1.id).join("voiceprints.json"),
            serde_json::to_string(&vp).unwrap(),
        )
        .unwrap();

        move_person_pack(&app, "Инженер 2", &p1.id, &p2.id).unwrap();

        assert!(absolute.exists());
        assert!(relative.exists());
    }

    #[test]
    fn corrupt_people_json_is_not_overwritten() {
        let (_g, app) = tmp_app("corrupt-people");
        let pack = create_pack(&app, "Отдел", "").unwrap();
        let file = app.join("packs").join(&pack.id).join("people.json");
        fs::write(&file, "{ broken").unwrap();

        assert!(add_person_to_pack(&app, &pack.id, json!({"name": "Новый"})).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ broken");
    }

    #[test]
    fn corrupt_target_voiceprints_blocks_move() {
        let (_g, app) = tmp_app("corrupt-target-vp");
        let p1 = create_pack(&app, "Отдел 1", "").unwrap();
        let p2 = create_pack(&app, "Отдел 2", "").unwrap();
        add_person_to_pack(&app, &p1.id, json!({"name": "Инженер 3"})).unwrap();
        let vp = voiceprints_v3(
            vec![json!({"name": "Инженер 3", "prototypes": []})],
            "eres2netv2-192d",
        );
        fs::write(
            app.join("packs").join(&p1.id).join("voiceprints.json"),
            serde_json::to_string(&vp).unwrap(),
        )
        .unwrap();
        let target = app.join("packs").join(&p2.id).join("voiceprints.json");
        fs::write(&target, "{ broken").unwrap();

        assert!(move_person_pack(&app, "Инженер 3", &p1.id, &p2.id).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "{ broken");
    }

    #[test]
    fn corrupt_people_json_blocks_move() {
        let (_g, app) = tmp_app("corrupt-move");
        let p1 = create_pack(&app, "Отдел 1", "").unwrap();
        let p2 = create_pack(&app, "Отдел 2", "").unwrap();
        let file = app.join("packs").join(&p1.id).join("people.json");
        fs::write(&file, "{ broken").unwrap();

        assert!(move_person_pack(&app, "Новый", &p1.id, &p2.id).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ broken");
    }

    #[test]
    fn add_and_move_person_between_packs() {
        let (_g, app) = tmp_app("move-person");
        let p1 = create_pack(&app, "Отдел 1", "").unwrap();
        let p2 = create_pack(&app, "Отдел 2", "").unwrap();

        add_person_to_pack(
            &app,
            &p1.id,
            json!({"name": "Инженер 1", "role": "Разработчик"}),
        )
        .unwrap();

        let pf1 = app.join("packs").join(&p1.id).join("people.json");
        let t1 = fs::read_to_string(&pf1).unwrap();
        assert!(t1.contains("Инженер 1"));

        move_person_pack(&app, "Инженер 1", &p1.id, &p2.id).unwrap();

        let t1_after = fs::read_to_string(&pf1).unwrap();
        assert!(!t1_after.contains("Инженер 1"));

        let pf2 = app.join("packs").join(&p2.id).join("people.json");
        let t2 = fs::read_to_string(&pf2).unwrap();
        assert!(t2.contains("Инженер 1"));
    }
}
