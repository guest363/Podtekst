//! Нарезки — личные данные, в репозиторий не попадают (P12).
//!
//! ```text
//! MR_DIAR_FIXTURES_DIR=./target/local-diar \
//!   cargo test -p mr-app --test diar_local \
//!   --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
//! ```

#![cfg(feature = "onnx")]

use std::path::{Path, PathBuf};

use mr_app_lib::{transcribe_file, TranscribeOptions};

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    sha256: String,
    channels: u16,
    expected_speakers: usize,
    #[serde(default)]
    probe_mark: Option<String>,
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
        let report = transcribe_file(&wav, &opts, &|pct, msg| {
            if pct % 10 == 0 {
                eprintln!("{}: {pct}% {msg}", case.id);
            }
        });
        let report = match report {
            Ok(report) => report,
            Err(error) => {
                failures.push(format!("{}: ошибка расшифровки: {error}", case.id));
                continue;
            }
        };
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
