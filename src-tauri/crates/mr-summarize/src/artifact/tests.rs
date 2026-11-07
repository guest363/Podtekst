use super::*;

fn utt(start: f64, end: f64, speaker: &str, text: &str) -> Utterance {
    Utterance {
        start,
        end,
        speaker: speaker.into(),
        text: text.into(),
    }
}

#[test]
fn digest_golden_vector() {
    let utterances = [
        utt(0.0, 1.5, "Спикер 1", "Привет, коллеги."),
        utt(1.5, 3.0, "Спикер 2", "Начнём."),
    ];
    assert_eq!(
        source_rev(&utterances),
        "df83afb242b2c7e81a47800cc198ae0f4634bf78fc4846846cd8dd09f52ab866"
    );
    assert_eq!(
        source_rev(&[]),
        "103b92e45253890820db806383c5f4c25ae009d8cbada5f175afac93ddd445d1"
    );
}

#[test]
fn digest_ignores_field_order_and_extra_fields() {
    let a = r#"{"utterances":[{"start":0.0,"end":1.0,"speaker":"A","text":"ok","words":[],"ctc_used":true}]}"#;
    let b = r#"{"meta":{"x":1},"utterances":[{"text":"ok","speaker":"A","end":1.0,"start":0.0,"silhouette":0.4}]}"#;
    let ta = Transcript::from_json(a).unwrap();
    let tb = Transcript::from_json(b).unwrap();
    assert_eq!(source_rev(&ta.utterances), source_rev(&tb.utterances));
    let tc = Transcript::from_json(
        r#"{"utterances":[{"start":0.0,"end":1.0,"speaker":"A","text":"ok!"}]}"#,
    )
    .unwrap();
    assert_ne!(source_rev(&ta.utterances), source_rev(&tc.utterances));
}

#[test]
fn digest_changes_on_speaker_rename() {
    let a = [utt(0.0, 1.0, "A", "text")];
    let b = [utt(0.0, 1.0, "B", "text")];
    assert_ne!(source_rev(&a), source_rev(&b));
}

#[test]
fn unsupported_schema_is_rejected() {
    let err = parse_artifact(r#"{"schema":"podtekst.summary/v2","schema_minor":0}"#).unwrap_err();
    assert!(matches!(err, SummaryError::SchemaUnsupported(_)));
    assert!(parse_artifact(r#"{"nope":true}"#).is_err());
}
