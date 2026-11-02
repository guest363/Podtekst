use hound::{SampleFormat, WavReader};

#[test]
fn live_recording_erle_report() {
    let Ok(path) = std::env::var("MR_ECHO_TEST_WAV") else {
        eprintln!("MR_ECHO_TEST_WAV не задан — пропуск");
        return;
    };
    let mut reader = WavReader::open(&path).expect("WAV читается");
    let spec = reader.spec();
    assert_eq!(spec.channels, 2, "нужна стерео-запись (микрофон / система)");
    assert_eq!(spec.sample_rate, 16000);
    assert_eq!(spec.sample_format, SampleFormat::Int);
    let mut channels: Vec<Vec<f32>> = {
        let mut left = Vec::new();
        let mut right = Vec::new();
        for s in reader.samples::<i16>() {
            let s = s.expect("сэмплы читаются") as f32 / 32767.0;
            if left.len() <= right.len() {
                left.push(s);
            } else {
                right.push(s);
            }
        }
        vec![left, right]
    };
    let duration = channels[0].len() as f32 / 16000.0;
    let mic_before: f32 = channels[0].iter().map(|x| x * x).sum();
    if mic_before <= 0.0 {
        eprintln!("запись {duration:.0} с без сигнала в микрофоне — пропуск");
        return;
    }
    let (left, right) = channels.split_at_mut(1);
    let report = mr_audio::echo::suppress_echo(&mut left[0], &right[0], 16000);
    let mic_after: f32 = channels[0].iter().map(|x| x * x).sum();
    println!(
        "запись {duration:.0} с: энергия микрофона {:.1} → {:.1} (×{:.3}), обработано={}, ERLE={:?} дБ",
        mic_before,
        mic_after,
        mic_after / mic_before,
        report.echo_windows,
        report.erle_db
    );
    assert!(mic_after.is_finite() && mic_after > 0.0);
    assert!(mic_after / mic_before > 0.1, "канал подавлен в катаклизм");
}
