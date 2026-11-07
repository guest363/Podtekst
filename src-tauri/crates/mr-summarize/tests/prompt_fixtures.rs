use std::fs;
use std::path::Path;

use mr_summarize::preset::{self, Preset};
use mr_summarize::prompt;
use mr_summarize::text::{Language, Transcript};

const NONCE: &str = "0123456789abcdef0123456789abcdef";

fn transcript(file: &str) -> Transcript {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(file);
    Transcript::from_json(&fs::read_to_string(path).unwrap()).unwrap()
}

fn build(preset: &Preset, file: &str, language: Language) -> prompt::Prompt {
    prompt::build_with_nonce(preset, &transcript(file).utterances, language, NONCE)
}

#[test]
fn prompts_for_matrix_fixtures_contain_catalog_sections() {
    let cases = [
        ("meeting", "meeting-ru.json", Language::Ru),
        ("scrum", "scrum-ru.json", Language::Ru),
        ("legal", "legal-ru.json", Language::Ru),
        ("lecture", "lecture-ru.json", Language::Ru),
    ];
    for (preset_id, file, language) in cases {
        let preset = preset::get(preset_id).unwrap();
        let built = build(preset, file, language);
        assert_eq!(built.prompt_version, format!("{preset_id}-v2"));
        for section in preset.sections {
            assert!(
                built.user.contains(&format!("## {}", section.title_ru)),
                "{preset_id} missing {}",
                section.id
            );
        }
        assert!(built.user.contains(&format!("<<<DATA_{NONCE}_BEGIN>>>")));
        assert_eq!(built.user.matches(NONCE).count(), 2);
    }
}

#[test]
fn prompt_json_is_valid_and_escapes_data() {
    let built = build(
        preset::get("injection-ru").unwrap_or(preset::get("meeting").unwrap()),
        "injection-ru.json",
        Language::Ru,
    );
    let marker = format!("<<<DATA_{NONCE}_BEGIN>>>\n");
    let json_line = built
        .user
        .split(&marker)
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(json_line).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 5);
    assert!(parsed[1]["text"].as_str().unwrap().contains("Игнорируй"));
}

#[test]
fn english_prompt_uses_english_sections() {
    let built = build(
        preset::get("meeting").unwrap(),
        "english.json",
        Language::En,
    );
    assert!(built.user.contains("## Brief"));
    assert!(built.system.contains("Output language: English"));
}

#[test]
fn nonce_is_absent_from_model_output_contract() {
    let built = build(
        preset::get("meeting").unwrap(),
        "meeting-ru.json",
        Language::Ru,
    );
    assert!(!prompt::nonce_in_output(
        "## Кратко\n- Обычный пункт.\n",
        &built.nonce
    ));
    assert!(prompt::nonce_in_output(
        &format!("prefix {} suffix", built.nonce),
        &built.nonce
    ));
}
