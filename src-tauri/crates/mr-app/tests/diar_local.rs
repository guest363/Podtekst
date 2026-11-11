//! Нарезки — личные данные, в репозиторий не попадают (P12).
//!
//! ```text
//! MR_DIAR_FIXTURES_DIR=./target/local-diar \
//!   cargo test -p mr-app --test diar_local \
//!   --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
//! ```

#![cfg(feature = "onnx")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mr_app_lib::{transcribe_file, TranscribeOptions};

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    sha256: String,
    channels: u16,
    expected_speakers: usize,
    #[serde(default)]
    probe_mark: Option<String>,
    #[serde(default)]
    expected_turns: Option<Vec<Turn>>,
    #[serde(default)]
    max_rtf: Option<f64>,
    #[serde(default)]
    identity_turns: Vec<Turn>,
}

#[derive(serde::Deserialize, Clone)]
struct Turn {
    start: f64,
    end: f64,
    speaker: String,
}

#[derive(serde::Deserialize)]
struct TranscriptUtterance {
    start: f64,
    end: f64,
    speaker: String,
}

#[derive(serde::Deserialize)]
struct Transcript {
    utterances: Vec<TranscriptUtterance>,
}

#[derive(Debug)]
struct DerMetrics {
    der: f64,
    missed: f64,
    false_alarm: f64,
    confusion: f64,
    total_ref: f64,
}

fn dominant<'a>(intervals: &[(&'a str, f64, f64)], start: f64, end: f64) -> Option<&'a str> {
    let mut best: Option<(&str, f64)> = None;
    for &(speaker, a, b) in intervals {
        let overlap = (end.min(b) - start.max(a)).max(0.0);
        if overlap <= 0.0 {
            continue;
        }
        if best.is_none_or(|(_, score)| overlap > score) {
            best = Some((speaker, overlap));
        }
    }
    best.map(|(speaker, _)| speaker)
}

fn wrong_identity_seconds(hyp: &[TranscriptUtterance], reference: &[Turn]) -> f64 {
    reference
        .iter()
        .map(|turn| {
            let mut intervals: Vec<(f64, f64)> = hyp
                .iter()
                .filter(|utterance| utterance.speaker == turn.speaker)
                .map(|utterance| (utterance.start.max(turn.start), utterance.end.min(turn.end)))
                .filter(|(start, end)| end > start)
                .collect();
            intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut covered = 0.0;
            let mut cursor = turn.start;
            for (start, end) in intervals {
                covered += (end - start.max(cursor)).max(0.0);
                cursor = cursor.max(end);
            }
            ((turn.end - turn.start).max(0.0) - covered).max(0.0)
        })
        .sum()
}

fn compute_der(hyp: &[TranscriptUtterance], reference: &[Turn], collar_sec: f64) -> DerMetrics {
    let hyp_intervals: Vec<(&str, f64, f64)> = hyp
        .iter()
        .map(|u| (u.speaker.as_str(), u.start, u.end))
        .collect();
    let ref_intervals: Vec<(&str, f64, f64)> = reference
        .iter()
        .map(|t| (t.speaker.as_str(), t.start, t.end))
        .collect();
    let mut edges: Vec<f64> = hyp
        .iter()
        .flat_map(|u| [u.start, u.end])
        .chain(reference.iter().flat_map(|t| [t.start, t.end]))
        .filter(|x| x.is_finite())
        .collect();
    edges.sort_by(f64::total_cmp);
    edges.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let ref_edges: Vec<f64> = reference
        .iter()
        .flat_map(|t| [t.start, t.end])
        .filter(|x| x.is_finite())
        .collect();
    let total_ref: f64 = reference.iter().map(|t| (t.end - t.start).max(0.0)).sum();
    let mut matrix: HashMap<(String, String), f64> = HashMap::new();
    let mut missed = 0.0;
    let mut false_alarm = 0.0;
    for pair in edges.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let duration = end - start;
        if duration <= 0.0 {
            continue;
        }
        let mid = (start + end) / 2.0;
        if ref_edges
            .iter()
            .any(|edge| (mid - edge).abs() <= collar_sec)
        {
            continue;
        }
        let hyp_speaker = dominant(&hyp_intervals, start, end);
        let ref_speaker = dominant(&ref_intervals, start, end);
        match (hyp_speaker, ref_speaker) {
            (Some(h), Some(r)) => {
                *matrix.entry((h.to_string(), r.to_string())).or_default() += duration;
            }
            (Some(_), None) => false_alarm += duration,
            (None, Some(_)) => missed += duration,
            (None, None) => {}
        }
    }
    let hyp_names: Vec<String> = {
        let mut names: Vec<String> = hyp.iter().map(|u| u.speaker.clone()).collect();
        names.sort();
        names.dedup();
        names
    };
    let ref_names: Vec<String> = {
        let mut names: Vec<String> = reference.iter().map(|t| t.speaker.clone()).collect();
        names.sort();
        names.dedup();
        names
    };
    let scores: Vec<Vec<f64>> = hyp_names
        .iter()
        .map(|h| {
            ref_names
                .iter()
                .map(|r| matrix.get(&(h.clone(), r.clone())).copied().unwrap_or(0.0))
                .collect()
        })
        .collect();
    let correct: f64 = mr_ml::linear_sum_assignment_max(&scores)
        .into_iter()
        .map(|(h, r)| scores[h][r])
        .sum();
    let confused: f64 = matrix.values().sum::<f64>() - correct;
    let der = if total_ref > 0.0 {
        (missed + false_alarm + confused) / total_ref
    } else {
        0.0
    };
    DerMetrics {
        der,
        missed,
        false_alarm,
        confusion: confused,
        total_ref,
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CasesFile {
    List(Vec<Case>),
    Object { cases: Vec<Case> },
}

fn load_cases(path: &Path) -> Vec<Case> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("не читается {}: {e}", path.display()));
    match serde_json::from_str::<CasesFile>(&text)
        .unwrap_or_else(|e| panic!("битый {}: {e}", path.display()))
    {
        CasesFile::List(cases) | CasesFile::Object { cases } => cases,
    }
}

#[test]
#[ignore = "нужны локальные нарезки в MR_DIAR_FIXTURES_DIR"]
fn local_diarization_cases() {
    let Some(dir) = std::env::var_os("MR_DIAR_FIXTURES_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        eprintln!("MR_DIAR_FIXTURES_DIR не задан — пропуск");
        return;
    };
    let cases = load_cases(&dir.join("cases.json"));
    assert!(!cases.is_empty(), "cases.json пуст");
    let mut failures = vec![];
    for case in cases {
        let wav = dir.join(format!("{}.wav", case.id));
        let actual =
            mr_models::sha256_of_file(&wav).unwrap_or_else(|e| panic!("{}: {e}", wav.display()));
        assert_eq!(actual, case.sha256, "{}: хеш не совпал", case.id);
        let reader = hound::WavReader::open(&wav).expect("WAV не открывается");
        let channels = reader.spec().channels;
        assert_eq!(channels, case.channels, "{}: число каналов", case.id);

        let opts = TranscribeOptions::default();
        let started = Instant::now();
        let report = transcribe_file(&wav, &opts, &|pct, msg| {
            if pct % 10 == 0 {
                eprintln!("{}: {pct}% {msg}", case.id);
            }
        });
        let elapsed = started.elapsed().as_secs_f64();
        let report = match report {
            Ok(report) => report,
            Err(error) => {
                failures.push(format!("{}: ошибка расшифровки: {error}", case.id));
                continue;
            }
        };
        let rtf = elapsed / report.duration_sec.max(1e-9);
        if let Some(max_rtf) = case.max_rtf {
            if rtf > max_rtf {
                failures.push(format!(
                    "{}: RTF {rtf:.3} выше предела {max_rtf:.3}",
                    case.id
                ));
            }
        }
        if case.expected_turns.is_some() || !case.identity_turns.is_empty() {
            match std::fs::read_to_string(&report.json_path)
                .ok()
                .and_then(|text| serde_json::from_str::<Transcript>(&text).ok())
            {
                Some(transcript) => {
                    let wrong =
                        wrong_identity_seconds(&transcript.utterances, &case.identity_turns);
                    if wrong > 0.0 {
                        failures.push(format!("{}: неверный профиль на {wrong:.2} с", case.id));
                    }
                    if let Some(reference) = &case.expected_turns {
                        let der = compute_der(&transcript.utterances, reference, 0.25);
                        eprintln!(
                            "{}: DER {:.3} (miss {:.2}, fa {:.2}, conf {:.2})",
                            case.id, der.der, der.missed, der.false_alarm, der.confusion
                        );
                        if der.total_ref <= 0.0 || der.der > 0.12 {
                            failures.push(format!(
                                "{}: DER {:.3} выше 0.12 (эталон {:.2} с)",
                                case.id, der.der, der.total_ref
                            ));
                        }
                    }
                }
                None => failures.push(format!("{}: не читается transcript.json", case.id)),
            }
        }
        eprintln!(
            "{}: спикеры {:?}, реплик {}, probe: {}",
            case.id,
            report.speakers,
            report.utterances,
            case.probe_mark.as_deref().unwrap_or("не задан")
        );
        if report.speakers.len() != case.expected_speakers {
            failures.push(format!(
                "{}: ожидалось спикеров {}, получено {} ({:?})",
                case.id,
                case.expected_speakers,
                report.speakers.len(),
                report.speakers
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn der_identical_turns_is_zero() {
    let reference = vec![Turn {
        start: 0.0,
        end: 5.0,
        speaker: "A".to_string(),
    }];
    let hyp = vec![TranscriptUtterance {
        start: 0.0,
        end: 5.0,
        speaker: "Спикер 1".to_string(),
    }];
    let der = compute_der(&hyp, &reference, 0.25);
    assert!(der.der < 1e-9, "{der:?}");
}

#[test]
fn der_counts_missed_and_false_alarm() {
    let reference = vec![Turn {
        start: 0.0,
        end: 10.0,
        speaker: "A".to_string(),
    }];
    let hyp = vec![
        TranscriptUtterance {
            start: 0.0,
            end: 5.0,
            speaker: "Спикер 1".to_string(),
        },
        TranscriptUtterance {
            start: 11.0,
            end: 12.0,
            speaker: "Спикер 2".to_string(),
        },
    ];
    let der = compute_der(&hyp, &reference, 0.25);
    assert!((der.missed - 5.0).abs() < 1e-9, "{der:?}");
    assert!((der.false_alarm - 1.0).abs() < 1e-9, "{der:?}");
    assert!((der.der - 0.6).abs() < 1e-9, "{der:?}");
}

#[test]
fn der_maps_swapped_labels() {
    let reference = vec![
        Turn {
            start: 0.0,
            end: 5.0,
            speaker: "A".to_string(),
        },
        Turn {
            start: 5.0,
            end: 10.0,
            speaker: "B".to_string(),
        },
    ];
    let hyp = vec![
        TranscriptUtterance {
            start: 0.0,
            end: 5.0,
            speaker: "X".to_string(),
        },
        TranscriptUtterance {
            start: 5.0,
            end: 10.0,
            speaker: "Y".to_string(),
        },
    ];
    let der = compute_der(&hyp, &reference, 0.25);
    assert!(der.der < 1e-9, "{der:?}");
}

#[test]
fn identity_check_rejects_swapped_profiles_even_when_der_is_zero() {
    let reference = vec![
        Turn {
            start: 0.0,
            end: 3.0,
            speaker: "A".to_string(),
        },
        Turn {
            start: 3.0,
            end: 6.0,
            speaker: "B".to_string(),
        },
    ];
    let hyp = vec![
        TranscriptUtterance {
            start: 0.0,
            end: 3.0,
            speaker: "B".to_string(),
        },
        TranscriptUtterance {
            start: 3.0,
            end: 6.0,
            speaker: "A".to_string(),
        },
    ];
    assert_eq!(compute_der(&hyp, &reference, 0.0).der, 0.0);
    assert_eq!(wrong_identity_seconds(&hyp, &reference), 6.0);
    assert_eq!(wrong_identity_seconds(&[], &reference), 6.0);
}
