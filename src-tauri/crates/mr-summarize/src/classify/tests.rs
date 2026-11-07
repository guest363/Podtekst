use super::*;
use crate::text::Utterance;

fn transcript(texts: &[(&str, &str)]) -> Transcript {
    Transcript {
        utterances: texts
            .iter()
            .map(|(speaker, text)| Utterance {
                start: 0.0,
                end: 1.0,
                speaker: (*speaker).into(),
                text: (*text).into(),
            })
            .collect(),
    }
}

#[test]
fn embedded_spec_matches_frozen_sha() {
    assert_eq!(spec_sha256(), SPEC_SHA256);
    assert_eq!(stopwords_sha256(), spec().token.stopwords_sha256);
    assert_eq!(spec().token.stopwords_total, 212);
    assert_eq!(spec().spec_version, SPEC_VERSION);
    assert_eq!(spec().heuristic_version, HEURISTIC_VERSION);
}

#[test]
fn source_validation_boundaries() {
    assert!(matches!(
        validate_source(&transcript(&[("A", "коротко")])),
        Err(SummaryError::TooShort { .. })
    ));
    let two = transcript(&[
        ("A", "Это достаточно длинное предложение для проверки."),
        ("B", "Да."),
    ]);
    assert!(validate_source(&two).unwrap().short);
    assert!(validate_source(&transcript(&[("A", "x")])).is_err());
    assert!(validate_source(&Transcript::default()).is_err());
}

#[test]
fn role_labels_use_speakers() {
    let t = transcript(&[
        ("Судья", "Слушаем стороны."),
        ("Юрист", "Готовы представить контррасчёт."),
    ]);
    let c = classify(&t);
    assert!(c.signals.contains(&Signal::ProceduralRoles));
}

#[test]
fn empty_signal_is_explicit() {
    let t = transcript(&[
            ("A", "На улице облачно и тихо, деревья стоят без движения, воздух прохладный, и ничего не происходит вокруг."),
            ("B", "В комнате горит лампа, на столе лежит бумага, часы показывают поздний вечер, и всё остаётся на местах."),
        ]);
    let c = classify(&t);
    assert_eq!(c.preset_id, None, "{c:?}");
    assert!(c.signals.contains(&Signal::NoSignal));
}
