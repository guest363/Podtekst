use std::collections::HashMap;
use std::path::PathBuf;

use mr_metrics::{analyze, Turn};

fn tone(freq: f32, samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|i| (std::f32::consts::TAU * freq * i as f32 / 16_000.0).sin() * 0.5)
        .collect()
}

fn temp_wav() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mr-metrics-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("fixture.wav")
}

#[test]
fn metrics_from_synthetic_stereo_wav() {
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
    let names = HashMap::new();
    let metrics = analyze(&turns, 11.0, Some(&path), 2, &names);

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
    assert!((f0_a - 200.0).abs() < 6.0, "f0 A={f0_a}");
    assert!((f0_b - 300.0).abs() < 8.0, "f0 B={f0_b}");

    let pause = metrics.navigation.first_long_pause.expect("pause");
    assert!((pause.start - 4.0).abs() < 0.01);
    assert!((pause.end - 9.0).abs() < 0.01);

    assert!(metrics.quality.bandwidth_hz.unwrap() < 1_000.0);
    assert!(metrics
        .warnings
        .iter()
        .any(|warning| warning == "narrow_band"));

    let _ = std::fs::remove_file(&path);
}
