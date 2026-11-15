use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mr_metrics::{analyze, Grade, Metrics, Turn};

fn tone(freq: f32, samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|i| {
            let t = i as f32 / 16_000.0;
            // 6,5 кГц держит полосу выше гейта просодии: чистый синус даёт narrow_band.
            (std::f32::consts::TAU * freq * t).sin() * 0.4
                + (std::f32::consts::TAU * 6_500.0 * t).sin() * 0.05
        })
        .collect()
}

fn temp_wav() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mr-metrics-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join(format!("fixture-{seq}.wav"))
}

fn fixture() -> (PathBuf, Vec<Turn>) {
    let path = temp_wav();
    let rate = 16_000;
    let total = 11 * rate;
    let mut left = vec![0.0_f32; total];
    let mut right = vec![0.0_f32; total];
    left[..2 * rate].copy_from_slice(&tone(200.0, 2 * rate));
    left[9 * rate..].copy_from_slice(&tone(200.0, 2 * rate));
    right[2 * rate..4 * rate].copy_from_slice(&tone(300.0, 2 * rate));
    mr_audio::write_stereo_wav(&path, &left, &right, 16_000).expect("wav");

    let turns = vec![
        Turn {
            start: 0.0,
            end: 2.0,
            speaker: "A".into(),
            channel: Some(0),
            text: "раз два три".into(),
        },
        Turn {
            start: 2.0,
            end: 4.0,
            speaker: "B".into(),
            channel: Some(1),
            text: "да нет".into(),
        },
        Turn {
            start: 9.0,
            end: 11.0,
            speaker: "A".into(),
            channel: Some(0),
            text: "четыре пять".into(),
        },
    ];
    (path, turns)
}

fn run(turns: &[Turn], path: &Path) -> Metrics {
    analyze(turns, 11.0, Some(path), 2, &HashMap::new())
}

#[test]
fn metrics_from_synthetic_stereo_wav() {
    let (path, turns) = fixture();
    let metrics = run(&turns, &path);

    assert!(
        (metrics.speech_sec - 6.0).abs() < 0.01,
        "{:?}",
        metrics.speech_sec
    );
    assert_eq!(metrics.speakers.len(), 2);
    let a = metrics.speakers.iter().find(|s| s.id == "A").expect("A");
    let b = metrics.speakers.iter().find(|s| s.id == "B").expect("B");
    assert!((a.share - 4.0 / 6.0).abs() < 0.01);
    assert!((b.share - 2.0 / 6.0).abs() < 0.01);

    let f0_a = a.f0_median_hz.expect("f0 A");
    let f0_b = b.f0_median_hz.expect("f0 B");
    assert!((f0_a - 200.0).abs() < 1.0, "f0 A={f0_a}");
    assert!((f0_b - 300.0).abs() < 1.0, "f0 B={f0_b}");

    let pause = metrics.navigation.first_long_pause.expect("pause");
    assert!((pause.start - 4.0).abs() < 0.01);
    assert!((pause.end - 9.0).abs() < 0.01);

    assert!(metrics.quality.bandwidth_hz.unwrap() >= 6_400.0);
    assert_eq!(metrics.quality.grade, Grade::Good, "{:?}", metrics.warnings);
    assert!(!metrics
        .warnings
        .iter()
        .any(|warning| warning == "narrow_band"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn clipping_marks_the_recording() {
    let path = temp_wav();
    let total = 3 * 16_000;
    mr_audio::write_stereo_wav(&path, &vec![1.0_f32; total], &vec![-1.0_f32; total], 16_000)
        .expect("wav");
    let turns = vec![Turn {
        start: 0.0,
        end: 3.0,
        speaker: "A".into(),
        channel: Some(0),
        text: "да".into(),
    }];
    let metrics = run(&turns, &path);

    assert_eq!(metrics.quality.clipping_ratio, Some(1.0));
    assert!(metrics.warnings.iter().any(|warning| warning == "clipping"));
    assert_ne!(metrics.quality.grade, Grade::Good);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn worst_channel_sets_snr_not_the_cleanest_one() {
    let rate = 16_000;
    let total = 4 * rate;
    let path_clean = temp_wav();
    let path_noisy = temp_wav();
    let mut clean_left = vec![0.0_f32; total];
    let mut noisy_right = vec![0.0_f32; total];
    for index in 0..total {
        let t = index as f32 / rate as f32;
        let speech = (std::f32::consts::TAU * 200.0 * t).sin() * 0.4
            + (std::f32::consts::TAU * 6_500.0 * t).sin() * 0.05;
        let quiet = (std::f32::consts::TAU * 1_100.0 * t).sin() * 0.002;
        clean_left[index] = if index < 2 * rate { speech } else { quiet };
        // Правый канал: та же речь, но в паузах слышен широкополосный шум.
        noisy_right[index] = if index < 2 * rate {
            speech * 0.8
        } else {
            (std::f32::consts::TAU * 1_100.0 * t).sin() * 0.2
        };
    }
    let turns = vec![
        Turn {
            start: 0.0,
            end: 2.0,
            speaker: "A".into(),
            channel: Some(0),
            text: "раз два три".into(),
        },
        Turn {
            start: 0.0,
            end: 2.0,
            speaker: "B".into(),
            channel: Some(1),
            text: "четыре пять".into(),
        },
    ];
    mr_audio::write_stereo_wav(&path_clean, &clean_left, &clean_left, 16_000).expect("wav");
    mr_audio::write_stereo_wav(&path_noisy, &clean_left, &noisy_right, 16_000).expect("wav");
    let clean = run(&turns, &path_clean).quality.snr_db.expect("snr");
    let noisy = run(&turns, &path_noisy).quality.snr_db.expect("snr");

    // reduce(min): шумный канал обязан ухудшить общий SNR. С reduce(max)
    // результат остался бы на уровне чистого канала.
    assert!(
        noisy < clean - 6.0,
        "snr clean={clean} noisy={noisy}: шумный канал спрятался за чистым"
    );
    let _ = std::fs::remove_file(&path_clean);
    let _ = std::fs::remove_file(&path_noisy);
}

#[test]
fn silent_second_channel_does_not_inflate_snr() {
    let path = temp_wav();
    let rate = 16_000;
    let total = 3 * rate;
    let mut left = vec![0.0_f32; total];
    for (index, sample) in left.iter_mut().enumerate() {
        let t = index as f32 / rate as f32;
        *sample = if index < 2 * rate {
            (std::f32::consts::TAU * 200.0 * t).sin() * 0.4
                + (std::f32::consts::TAU * 6_500.0 * t).sin() * 0.05
        } else {
            (std::f32::consts::TAU * 1_100.0 * t).sin() * 0.002
        };
    }
    mr_audio::write_stereo_wav(&path, &left, &vec![0.0_f32; total], 16_000).expect("wav");
    let turns = vec![Turn {
        start: 0.0,
        end: 2.0,
        speaker: "A".into(),
        channel: Some(0),
        text: "раз два три".into(),
    }];
    let metrics = run(&turns, &path);

    let snr = metrics.quality.snr_db.expect("snr");
    assert!(snr <= 60.0, "snr={snr}");
    assert!(metrics.quality.channel_balance_db.is_none());
    assert!(metrics
        .warnings
        .iter()
        .all(|warning| warning != "channel_balance"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn metrics_are_byte_identical_on_repeat() {
    let (path, turns) = fixture();
    let first = serde_json::to_vec(&run(&turns, &path)).expect("json");
    let second = serde_json::to_vec(&run(&turns, &path)).expect("json");
    assert_eq!(first, second, "тест-ретест разошёлся");
    let _ = std::fs::remove_file(&path);
}
