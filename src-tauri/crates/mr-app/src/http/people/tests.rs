use super::*;
use mr_packs as packs;
use serde_json::json;

use super::lists::{ensure_person_in_pack, resolve_target_pack};
use super::transcript_names::rename_speaker_in_files;
use super::voiceprints::{
    prototypes_centroid, remove_prototype, store_voiceprint, PrototypeSample,
};
use crate::config::app_data_dir;
use crate::voiceprints::{l2_normalize, EMBEDDING_DIM, MAX_PROTOTYPES};
fn profile_with(two: bool) -> serde_json::Value {
    let a: Vec<f64> = (0..EMBEDDING_DIM).map(|_| 1.0).collect();
    let b: Vec<f64> = (0..EMBEDDING_DIM).map(|_| 0.5).collect();
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
    (0..EMBEDDING_DIM).map(|_| 0.1).collect()
}

#[test]
fn remove_prototype_recomputes_centroid() {
    let mut p = profile_with(true);
    let (sn, empty) = remove_prototype(&mut p, "p1").expect("p1 обязан найтись");
    assert!(sn.is_none());
    assert!(!empty);
    assert_eq!(p["samples_count"], 1);
    let emb: Vec<f64> = serde_json::from_value(p["embedding"].clone()).unwrap();
    assert_eq!(emb.len(), EMBEDDING_DIM);
    // Остался один прототип из 0.5: центроид — его нормаль, а не старое среднее.
    let norm: f64 = emb.iter().map(|x| x * x).sum::<f64>().sqrt();
    assert!((norm - 1.0).abs() < 1e-9);
    assert!((emb[0] - emb[100]).abs() < 1e-12);
}

#[test]
fn store_voiceprint_centroid_follows_retained_prototypes() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-voiceprint-centroid-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

    let name = "Тест-Дрейф";
    for i in 0..15 {
        let mut v = vec![1.0; EMBEDDING_DIM];
        v[i] = 20.0;
        store_voiceprint(
            name,
            Some("default"),
            l2_normalize(&v),
            PrototypeSample {
                start: Some(1.0),
                end: Some(3.0),
                ..Default::default()
            },
        )
        .expect("прототип сохраняется");
    }
    let vp = load_all_voiceprints()
        .into_iter()
        .find(|v| same_name(v.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
        .expect("профиль сохранён");
    let protos = vp["prototypes"].as_array().expect("prototypes");
    assert_eq!(protos.len(), MAX_PROTOTYPES);
    assert_eq!(vp["samples_count"], MAX_PROTOTYPES as u64);
    assert!((protos[0]["duration_sec"].as_f64().unwrap() - 2.0).abs() < 1e-9);
    let stored: Vec<f64> = serde_json::from_value(vp["embedding"].clone()).unwrap();
    let (expected, count) = prototypes_centroid(protos);
    assert_eq!(count, MAX_PROTOTYPES);
    for (a, b) in stored.iter().zip(expected.iter()) {
        assert!((a - b).abs() < 1e-12, "центроид не совпал: {a} vs {b}");
    }

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snippet_path_never_escapes_pack() {
    let dir = std::env::temp_dir().join(format!(
        "mr-snippet-path-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(dir.join("snippets")).unwrap();
    std::fs::write(dir.join("snippets/a.wav"), b"x").unwrap();
    assert!(packs::safe_snippet_path(&dir, "snippets/a.wav").is_some());
    assert!(packs::safe_snippet_path(&dir, "../a.wav").is_none());
    assert!(packs::safe_snippet_path(&dir, "/etc/passwd").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn voiceprint_outlier_is_rejected() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-voiceprint-outlier-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);
    let name = "Тест-Выброс";
    for i in 0..3 {
        let mut v = vec![0.0; EMBEDDING_DIM];
        v[i] = 1.0;
        store_voiceprint(name, Some("default"), v, PrototypeSample::default())
            .expect("свой прототип");
    }
    let mut alien = vec![0.0; EMBEDDING_DIM];
    alien[100] = 1.0;
    let error = store_voiceprint(name, Some("default"), alien, PrototypeSample::default())
        .expect_err("чужой голос должен быть отклонён");
    assert!(error.contains("другой голос"), "{error}");
    let vp = load_all_voiceprints()
        .into_iter()
        .find(|v| same_name(v.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
        .expect("профиль");
    assert_eq!(vp["prototypes"].as_array().map(Vec::len), Some(3));

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_last_prototype_empties_profile() {
    let mut p = profile_with(false);
    let (_, empty) = remove_prototype(&mut p, "p1").expect("p1 обязан найтись");
    assert!(empty);
    assert!(remove_prototype(&mut p, "nope").is_none());
}

#[test]
fn target_pack_routing_and_people_sync() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-pack-routing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

    let custom = mr_packs::create_pack(&dir, "Команда Альфа", "").unwrap();
    ensure_person_in_pack("Инженер 1", Some("Разработчик"), &custom.id);

    assert_eq!(resolve_target_pack("Инженер 1", None), custom.id);
    assert_eq!(resolve_target_pack("Новый", None), "default");
    assert_eq!(resolve_target_pack("Новый", Some(&custom.id)), custom.id);
    assert_eq!(
        resolve_target_pack("Новый", Some(&dir.to_string_lossy())),
        "default"
    );
    assert_eq!(resolve_target_pack("Новый", Some("../evil")), "default");

    let custom2 = mr_packs::create_pack(&dir, "Команда Бета", "").unwrap();
    let ok = update_people(
        "Инженер 1",
        "Инженер 1 (Старший)",
        Some("Лид"),
        Some(&custom2.id),
        Some(&custom.id),
    );
    assert!(ok);
    assert_eq!(resolve_target_pack("Инженер 1 (Старший)", None), custom2.id);
    let p1_file = dir.join("packs").join(&custom.id).join("people.json");
    let t1 = std::fs::read_to_string(&p1_file).unwrap_or_default();
    assert!(!t1.contains("Инженер 1"));
    let p2_file = dir.join("packs").join(&custom2.id).join("people.json");
    let t2 = std::fs::read_to_string(&p2_file).unwrap_or_default();
    assert!(t2.contains("Инженер 1 (Старший)"));
    assert!(t2.contains("Лид"));

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_person_deletes_voiceprints_and_prevents_resurrection() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-remove-person-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

    let person_name = "Участник 1";
    ensure_person_in_pack(person_name, Some("Спикер"), "default");
    let emb = l2_normalize(
        &(0..EMBEDDING_DIM)
            .map(|i| (i + 1) as f64)
            .collect::<Vec<_>>(),
    );
    store_voiceprint(
        person_name,
        Some("default"),
        emb,
        PrototypeSample::default(),
    )
    .expect("голос сохраняется");

    let vps_before = load_all_voiceprints();
    assert!(vps_before.iter().any(|v| same_name(
        v.get("name").and_then(|x| x.as_str()).unwrap_or(""),
        person_name
    )));

    let ok = remove_person(person_name, None);
    assert!(ok);

    let vps_after = load_all_voiceprints();
    assert!(!vps_after.iter().any(|v| same_name(
        v.get("name").and_then(|x| x.as_str()).unwrap_or(""),
        person_name
    )));

    let p_file = dir.join("packs/default/people.json");
    let text = std::fs::read_to_string(&p_file).unwrap_or_default();
    assert!(!text.contains(person_name));

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_person_in_one_pack_keeps_same_name_in_other_pack() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-people-scoped-remove-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

    let first = packs::create_pack(&app_data_dir(), "Пакет 1", "")
        .expect("первый пакет создан")
        .id;
    let second = packs::create_pack(&app_data_dir(), "Пакет 2", "")
        .expect("второй пакет создан")
        .id;
    let person_name = "Одноимённый участник";
    ensure_person_in_pack(person_name, Some("Участник"), &first);
    ensure_person_in_pack(person_name, Some("Участник"), &second);

    assert!(remove_person(person_name, Some(&first)));

    let root = packs::packs_root_dir(&app_data_dir());
    let first_people = read_people_list(&root.join(&first).join("people.json")).unwrap_or_default();
    let second_people =
        read_people_list(&root.join(&second).join("people.json")).unwrap_or_default();
    assert_eq!(first_people.len(), 0, "{first_people:?}");
    assert_eq!(second_people.len(), 1, "{second_people:?}");

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_voiceprint_evicts_lowest_quality_prototype() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-voiceprint-evict-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

    let name = "Тест-Вытеснение";
    for i in 0..10 {
        let mut v = vec![1.0; EMBEDDING_DIM];
        v[i] = 10.0;
        store_voiceprint(
            name,
            Some("default"),
            l2_normalize(&v),
            PrototypeSample {
                start: Some(0.0),
                end: Some(4.0),
                quality: Some(1.0),
                ..Default::default()
            },
        )
        .expect("прототип");
    }
    let mut low = vec![1.0; EMBEDDING_DIM];
    low[10] = 10.0;
    store_voiceprint(
        name,
        Some("default"),
        l2_normalize(&low),
        PrototypeSample {
            start: Some(0.0),
            end: Some(1.0),
            quality: Some(0.3),
            ..Default::default()
        },
    )
    .expect("слабый прототип");

    let vp = load_all_voiceprints()
        .into_iter()
        .find(|v| same_name(v.get("name").and_then(|x| x.as_str()).unwrap_or(""), name))
        .expect("профиль");
    let protos = vp["prototypes"].as_array().expect("prototypes");
    assert_eq!(protos.len(), MAX_PROTOTYPES);
    assert!(
        protos.iter().all(|p| {
            let q = p["quality"].as_f64().unwrap_or(0.0);
            (q - 1.0).abs() < 1e-6
        }),
        "слабый прототип должен быть вытеснен первым"
    );

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_voiceprints_block_save_without_wipe() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-voiceprint-corrupt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

    store_voiceprint(
        "Тест-Сохранение",
        Some("default"),
        l2_normalize(&vec![1.0; EMBEDDING_DIM]),
        PrototypeSample {
            start: Some(0.0),
            end: Some(2.0),
            quality: Some(1.0),
            ..Default::default()
        },
    )
    .expect("первый профиль");
    let vp_file = dir.join("packs/default/voiceprints.json");
    std::fs::write(&vp_file, "{ broken").expect("порча файла");

    let error = store_voiceprint(
        "Тест-Новый",
        Some("default"),
        l2_normalize(&vec![0.5; EMBEDDING_DIM]),
        PrototypeSample::default(),
    )
    .expect_err("битый файл обязан блокировать запись");
    assert!(error.contains("повреждён"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&vp_file).expect("файл на месте"),
        "{ broken",
        "битый файл не должен перезаписываться"
    );

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn foreign_embedding_space_blocks_save_without_wipe() {
    let _guard = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "mr-voiceprint-space-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let prev = std::env::var("MEETING_RECORDER_DATA_DIR").ok();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);

    let vp_file = dir.join("packs/default/voiceprints.json");
    std::fs::create_dir_all(vp_file.parent().expect("каталог пакета")).expect("каталог");
    let foreign = json!({
        "version": 3,
        "embedding_space": "other-space",
        "profiles": [{"name": "Старый Голос", "embedding": []}],
    });
    std::fs::write(&vp_file, serde_json::to_string(&foreign).expect("json")).expect("запись");

    let error = store_voiceprint(
        "Тест-Новый",
        Some("default"),
        l2_normalize(&vec![0.5; EMBEDDING_DIM]),
        PrototypeSample::default(),
    )
    .expect_err("чужое пространство обязано блокировать запись");
    assert!(error.contains("другой моделью"), "{error}");
    let text = std::fs::read_to_string(&vp_file).expect("файл на месте");
    assert!(text.contains("Старый Голос"), "профили не должны стираться");

    if let Some(p) = prev {
        std::env::set_var("MEETING_RECORDER_DATA_DIR", p);
    } else {
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rename_updates_both_cards_and_returns_state() {
    let dir = std::env::temp_dir().join(format!("mr-rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("каталог");
    let audio = dir.join("meeting.wav");
    std::fs::write(&audio, b"").expect("файл");
    let data = json!({
        "meta": {
            "speakers": ["Собеседник 1", "Сергей Золотов"],
            "speakers_info": [
                {"name": "Собеседник 1", "matched": true, "matched_name": "Сергей Золотов"},
                {"name": "Сергей Золотов", "matched": true, "matched_name": "Сергей Золотов"}
            ]
        },
        "utterances": [
            {"start": 0.0, "end": 1.0, "text": "раз", "speaker": "Собеседник 1", "channel": 1},
            {"start": 1.0, "end": 2.0, "text": "два", "speaker": "Сергей Золотов", "channel": 1}
        ],
        "stats": {}
    });
    std::fs::write(
        dir.join("meeting.transcript.json"),
        serde_json::to_string_pretty(&data).expect("json"),
    )
    .expect("запись");

    let state = rename_speaker_in_files(&audio, "Сергей Петров", Some("Сергей Золотов"))
        .expect("состояние");
    let speakers: Vec<&str> = state["utterances"]
        .as_array()
        .expect("реплики")
        .iter()
        .map(|u| u["speaker"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(speakers, vec!["Сергей Петров", "Сергей Петров"]);
    let cards = state["speakers_info"].as_array().expect("карточки");
    assert!(cards.iter().all(|c| c["name"] == "Сергей Петров"));
    let markdown = std::fs::read_to_string(dir.join("meeting.md")).expect("md");
    assert!(markdown.contains("Сергей Петров"), "{markdown}");
    assert!(!markdown.contains("Собеседник 1"), "{markdown}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rename_without_source_keeps_transcript() {
    let dir = std::env::temp_dir().join(format!("mr-rename-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("каталог");
    let audio = dir.join("meeting.wav");
    std::fs::write(&audio, b"").expect("файл");
    let json_p = dir.join("meeting.transcript.json");
    let data = json!({
        "meta": {"speakers": ["А"], "speakers_info": [{"name": "А"}]},
        "utterances": [
            {"start": 0.0, "end": 1.0, "text": "раз", "speaker": "А", "channel": 0}
        ],
        "stats": {}
    });
    let before = serde_json::to_string_pretty(&data).expect("json");
    std::fs::write(&json_p, &before).expect("запись");
    assert!(rename_speaker_in_files(&audio, "Б", None).is_none());
    assert_eq!(std::fs::read_to_string(&json_p).expect("json"), before);
    let _ = std::fs::remove_dir_all(&dir);
}
