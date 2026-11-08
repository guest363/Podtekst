use mr_summarize::artifact::{self, Warning, MAX_JSON_BYTES};
use mr_summarize::SummaryError;

const ARTIFACT: &str = include_str!("golden/summary-artifact.json");
const RECORD: &str = include_str!("golden/summary-record.json");

fn artifact() -> mr_summarize::SummaryArtifact {
    artifact::parse_artifact(ARTIFACT).unwrap()
}

#[test]
fn dto_projection_matches_golden_field_by_field() {
    let artifact = artifact();
    let expected: serde_json::Value = serde_json::from_str(RECORD).unwrap();
    let actual =
        serde_json::to_value(artifact.project("records/synthetic-meeting-01.json")).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual["job_id"], actual["id"]);
    assert_eq!(actual["markdown"].as_str().unwrap(), artifact.to_markdown());
    assert_eq!(actual["source_rev"], artifact.source.rev);
}

#[test]
fn artifact_roundtrip_and_additive_fields_are_ignored() {
    let artifact = artifact();
    let json = artifact.to_json().unwrap();
    let parsed = artifact::parse_artifact(&json).unwrap();
    assert_eq!(parsed, artifact);

    let mut value: serde_json::Value = serde_json::from_str(ARTIFACT).unwrap();
    value["future_additive"] = serde_json::json!({"x": 1});
    let with_future = artifact::parse_artifact(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(with_future, artifact);
}

#[test]
fn refs_and_quotes_are_not_serialized() {
    let artifact = artifact();
    let json = artifact.to_json().unwrap();
    assert!(!json.contains("\"refs\""));
    assert!(!json.contains("\"quote\""));
    assert!(!json.contains("\"deadline_ref\""));
    assert!(!artifact.to_markdown().contains("[00:"));
}

#[test]
fn optional_sections_drop_before_required() {
    let mut artifact = artifact();
    artifact
        .summary
        .sections
        .push(mr_summarize::SummarySection {
            id: "questions".into(),
            title: "Открытые вопросы".into(),
            role: mr_summarize::SectionRole::Plain,
            items: vec![mr_summarize::SummaryItem {
                text: "я".repeat(MAX_JSON_BYTES + 1024),
                role: mr_summarize::SectionRole::Plain,
                task: None,
            }],
        });
    artifact.summary.items_total = 4;
    artifact.enforce_caps().unwrap();
    assert!(artifact
        .summary
        .sections
        .iter()
        .all(|s| s.id != "questions"));
    assert!(artifact.summary.sections.iter().any(|s| s.id == "brief"));
    assert!(artifact.summary.sections.iter().any(|s| s.id == "tasks"));
    assert_eq!(artifact.summary.items_total, 3);
}

#[test]
fn oversized_required_content_is_output_capped() {
    let mut artifact = artifact();
    artifact.summary.sections[0].items[0].text = "я".repeat(MAX_JSON_BYTES + 1024);
    let err = artifact.enforce_caps().unwrap_err();
    assert!(matches!(err, SummaryError::OutputCapped));
}

#[test]
fn warnings_serialize_to_contract_names() {
    let names = [
        (Warning::ShortSource, "short_source"),
        (Warning::DeadlineRecovered, "deadline_recovered"),
        (Warning::ParserRecovered, "parser_recovered"),
        (Warning::InjectionSuspected, "injection_suspected"),
        (Warning::DetailLongShortSource, "detail_long_short_source"),
    ];
    for (warning, expected) in names {
        assert_eq!(serde_json::to_value(warning).unwrap(), expected);
    }
}
