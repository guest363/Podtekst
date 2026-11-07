use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use mr_summarize::classify::{self, Confidence};
use mr_summarize::text::Transcript;

const GOLDEN: &str = include_str!("golden/classify-golden.v1.json");

#[test]
fn classifier_matches_frozen_golden_scores() {
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(golden["golden_version"], 1);
    assert_eq!(golden["spec_version"], classify::SPEC_VERSION);
    assert_eq!(golden["heuristic_version"], classify::HEURISTIC_VERSION);

    let cases = golden["cases"].as_array().unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    assert_eq!(cases.len(), 8);

    for expected in cases {
        let id = expected["id"].as_str().unwrap();
        let json = fs::read_to_string(dir.join(format!("{id}.json"))).unwrap();
        let transcript = Transcript::from_json(&json).unwrap();
        let actual = classify::classify(&transcript);
        assert_eq!(
            serde_json::to_value(&actual.scores).unwrap(),
            expected["scores"],
            "scores for {id}"
        );
        assert_eq!(
            serde_json::to_value(&actual.preset_id).unwrap(),
            expected["preset_id"],
            "preset for {id}"
        );
        assert_eq!(
            serde_json::to_value(actual.confidence).unwrap(),
            expected["confidence"],
            "confidence for {id}"
        );
        assert_eq!(
            actual.margin,
            expected["margin"].as_u64().unwrap() as u32,
            "margin for {id}"
        );
        assert_eq!(
            actual.ambiguous,
            expected["ambiguous"].as_bool().unwrap(),
            "ambiguous for {id}"
        );
        assert_eq!(
            serde_json::to_value(&actual.signals).unwrap(),
            expected["signals"],
            "signals for {id}"
        );
        assert_eq!(
            serde_json::to_value(actual.style).unwrap(),
            expected["style"],
            "style for {id}"
        );
        assert_eq!(
            serde_json::to_value(actual.language).unwrap(),
            expected["language"],
            "language for {id}"
        );
    }
}

#[test]
fn golden_scores_are_the_frozen_spec_table() {
    let table: BTreeMap<&str, [u32; 6]> = BTreeMap::from([
        ("meeting-ru", [9, 3, 5, 0, 2, 4]),
        ("scrum-ru", [8, 11, 2, 0, 2, 2]),
        ("legal-ru", [4, 1, 10, 0, 1, 2]),
        ("lecture-ru", [4, 3, 1, 9, 2, 1]),
        ("psychology-ru", [1, 2, 1, 2, 9, 3]),
        ("personal-ru", [4, 3, 2, 2, 2, 6]),
        ("english", [9, 5, 1, 1, 2, 3]),
        ("injection-ru", [8, 3, 1, 1, 2, 2]),
    ]);
    let order = [
        "meeting",
        "scrum",
        "legal",
        "lecture",
        "psychology",
        "personal",
    ];
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
    for case in golden["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let expected = table[id];
        let actual: Vec<u32> = order
            .iter()
            .map(|p| case["scores"][p].as_u64().unwrap() as u32)
            .collect();
        assert_eq!(actual, expected, "frozen table for {id}");
    }
}

#[test]
fn sensitive_primary_requires_high_confidence_and_base() {
    let json = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legal-ru.json"),
    )
    .unwrap();
    let transcript = Transcript::from_json(&json).unwrap();
    let result = classify::classify(&transcript);
    assert_eq!(result.preset_id.as_deref(), Some("legal"));
    assert_eq!(result.confidence, Some(Confidence::High));
    assert!(result.base_scores["legal"] >= 4);
    assert!(!result.sensitive_suppressed);
}

#[test]
fn scrum_meeting_takeover_is_marked_ambiguous() {
    let transcript = Transcript::from_json(
        r#"{"utterances":[
            {"start":0,"end":1,"speaker":"A","text":"Согласовали план, надо вопрос."},
            {"start":1,"end":2,"speaker":"B","text":"Спринт и стендап. Бэклог, релиз и тестирование на следующей неделе, детали позже в рабочем порядке."}
        ]}"#,
    )
    .unwrap();
    let result = classify::classify(&transcript);
    assert_eq!(result.preset_id.as_deref(), Some("meeting"));
    assert!(result.ambiguous);
    assert!(result.alternatives.contains(&"scrum".to_owned()));
}
