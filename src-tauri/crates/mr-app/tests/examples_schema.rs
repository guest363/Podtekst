//! Примеры из `examples/` никто не читает в рантайме, поэтому их дрейф
//! относительно реальных загрузчиков ловится только здесь.

use std::path::Path;

fn example(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Ровно те ключи, которые читает `config.rs` (плюс служебные счётчики).
const CONFIG_KEYS: [&str; 11] = [
    "save_dir",
    "record_mode",
    "record_mode_explicit",
    "auto_transcribe",
    "hotwords",
    "ctc_enabled",
    "diar",
    "transcribed_count",
    "total_saved_minutes",
    "donate_prompt_dismissed",
    "skipped_update_version",
];

#[test]
fn config_example_uses_real_settings_keys() {
    let config: serde_json::Value =
        serde_json::from_str(&example("config.example.json")).expect("config.example.json: JSON");
    let object = config.as_object().expect("объект настроек");
    for key in object.keys() {
        assert!(
            CONFIG_KEYS.contains(&key.as_str()),
            "ключ {key} не читается config.rs: пользователь получит тихий no-op"
        );
    }
    assert!(
        object.get("engine").is_none(),
        "движок задаётся телом POST /api/transcribe/start, а не конфигом"
    );
}

#[test]
fn people_example_matches_people_json_schema() {
    let people: serde_json::Value =
        serde_json::from_str(&example("people.example.json")).expect("people.example.json: JSON");
    let list = people.as_array().expect("список участников");
    assert!(!list.is_empty());
    for person in list {
        let object = person.as_object().expect("участник — объект");
        assert!(
            object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| !name.trim().is_empty()),
            "у участника должно быть имя"
        );
        assert!(
            object.get("domain").is_none(),
            "domain подставляет сервер (id пакета), в файле он игнорируется"
        );
        for key in object.keys() {
            assert!(
                matches!(key.as_str(), "name" | "role" | "is_external"),
                "неизвестный ключ участника: {key}"
            );
        }
    }
}

#[test]
fn voicepack_example_keeps_documented_format() {
    let pack: serde_json::Value =
        serde_json::from_str(&example("demo.voicepack.json")).expect("demo.voicepack.json: JSON");
    assert_eq!(pack["format"], "meeting-recorder-voicepack-v2");
    assert!(pack["manifest"]["id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert_eq!(pack["voiceprints"]["version"], 3);
}
