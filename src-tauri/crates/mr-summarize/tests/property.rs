use mr_summarize::artifact;
use mr_summarize::chunk;
use mr_summarize::classify;
use mr_summarize::parse;
use mr_summarize::preset;
use mr_summarize::text::{Transcript, Utterance};

fn utterance(i: usize, chars: usize) -> Utterance {
    Utterance {
        start: i as f64,
        end: i as f64 + 1.0,
        speaker: format!("S{}", i % 3),
        text: "я".repeat(chars),
    }
}

#[test]
fn chunk_coverage_is_exact_and_bounded() {
    let mut seed = 42u64;
    for _ in 0..100 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let count = (seed % 40) as usize + 1;
        let transcript = Transcript {
            utterances: (0..count)
                .map(|i| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    utterance(i, (seed % 9_000) as usize + 1)
                })
                .collect(),
        };
        let plan = chunk::plan(&transcript, chunk::MAX_CHUNKS);
        assert!(plan.chunks.len() <= chunk::MAX_CHUNKS);
        assert!((0.0..=1.0).contains(&plan.coverage));
        if !plan.truncated {
            assert!((plan.coverage - 1.0).abs() < f64::EPSILON);
        }
        for pair in plan.chunks.windows(2) {
            assert!(pair[0].end <= pair[1].end);
            assert!(pair[0].start < pair[0].end);
        }
    }
}

#[test]
fn parser_handles_utf8_and_never_panics() {
    let output = "## Кратко\n- Привет 👋, мир.\n- Ещё пункт.\n";
    let parsed = parse::parse(output, preset::get("meeting").unwrap()).unwrap();
    assert_eq!(parsed.sections[0].items[0].text, "Привет 👋, мир.");
}

#[test]
fn digest_is_stable_across_repeated_calls() {
    let transcript = Transcript {
        utterances: vec![
            utterance(0, 10),
            Utterance {
                start: 1.0,
                end: 2.0,
                speaker: "Спикер 1".into(),
                text: "Привет, коллеги.".into(),
            },
        ],
    };
    let a = artifact::source_rev(&transcript.utterances);
    let b = artifact::source_rev(&transcript.utterances.clone());
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
}

#[test]
fn mixed_language_is_other_not_ru_or_en() {
    let transcript = Transcript {
        utterances: vec![
            Utterance {
                start: 0.0,
                end: 1.0,
                speaker: "A".into(),
                text: "Hello colleagues, fine thanks, how are you, как дела?".into(),
            },
            Utterance {
                start: 1.0,
                end: 2.0,
                speaker: "B".into(),
                text: "Fine, спасибо.".into(),
            },
        ],
    };
    assert_eq!(
        classify::classify(&transcript).language,
        mr_summarize::text::Language::Other
    );
}

#[test]
fn source_rev_changes_when_only_text_changes() {
    let a = Transcript {
        utterances: vec![utterance(0, 10)],
    };
    let b = Transcript {
        utterances: vec![Utterance {
            text: "я".repeat(11),
            ..utterance(0, 10)
        }],
    };
    assert_ne!(
        artifact::source_rev(&a.utterances),
        artifact::source_rev(&b.utterances)
    );
}

#[test]
fn artifact_json_roundtrip_is_stable() {
    let artifact = artifact::parse_artifact(include_str!("golden/summary-artifact.json")).unwrap();
    let first = artifact.to_json().unwrap();
    let second = artifact::parse_artifact(&first).unwrap().to_json().unwrap();
    assert_eq!(first, second);
}

#[test]
#[ignore = "release-only performance gate; run with MR_PERF=1 and --release"]
fn classify_100k_p95() {
    if std::env::var("MR_PERF").as_deref() != Ok("1") {
        return;
    }
    let transcript = Transcript {
        utterances: (0..200)
            .map(|i| Utterance {
                start: i as f64,
                end: i as f64 + 1.0,
                speaker: format!("S{}", i % 3),
                text: "Обсудили сроки и задачи, согласовали план запуска, проверили вопросы и решения.".repeat(7),
            })
            .collect(),
    };
    let chars = transcript.total_chars();
    assert!(chars >= 100_000);
    let mut timings = Vec::new();
    for _ in 0..100 {
        let started = std::time::Instant::now();
        let _ = classify::classify(&transcript);
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    let p50 = timings[timings.len() / 2];
    let p95 = timings[(timings.len() as f64 * 0.95) as usize - 1];
    eprintln!("classify 100k chars: p50={p50:.2} ms p95={p95:.2} ms");
    assert!(p95 < 5.0, "p95 {p95} ms exceeds 5 ms");
}
