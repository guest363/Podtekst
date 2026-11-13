use mr_audio::dsp::{agc_analysis_16k, agc_apply_16k, agc_rms_16k};

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

fn signal(amplitude: f32, seconds: usize) -> Vec<f32> {
    (0..seconds * 16_000)
        .map(|i| amplitude * (std::f32::consts::TAU * 440.0 * i as f32 / 16_000.0).sin())
        .collect()
}

#[test]
fn quiet_speech_reaches_target_without_amplifying_pauses() {
    let mut samples = signal(0.0001, 1);
    samples.extend(signal(0.01, 4));
    samples.extend(signal(0.0001, 1));
    let stats = agc_rms_16k(&mut samples);
    let speech_rms = rms(&samples[32_000..64_000]);
    assert!((0.085..0.11).contains(&speech_rms), "{speech_rms}");
    assert!(rms(&samples[..8_000]) < 0.0001);
    assert!(rms(&samples[88_000..]) < 0.0001);
    assert!(stats.gain_p95 > 10.0);
    assert!((0.6..0.75).contains(&stats.amplified_fraction));
}

#[test]
fn level_frontend_is_bounded_and_accepts_silence() {
    let mut samples = vec![0.0; 16_000];
    agc_rms_16k(&mut samples);
    assert!(samples.iter().all(|&value| value == 0.0));
    samples.extend(signal(0.00001, 2));
    let stats = agc_rms_16k(&mut samples);
    assert!(stats.gain_p95 <= 100.0);
    assert!(rms(&samples[16_000..]) <= 0.00071);
    assert!(samples
        .iter()
        .all(|value| value.is_finite() && value.abs() <= 1.0));
    agc_rms_16k(&mut []);
    let mut full_scale: Vec<f32> = (0..16_000)
        .map(|i| if i % 64 < 32 { 1.0 } else { -1.0 })
        .collect();
    agc_rms_16k(&mut full_scale);
    assert!(full_scale.iter().all(|value| value.abs() <= 1.0));
}

#[test]
fn steady_background_is_not_promoted_to_speech() {
    let mut samples = signal(0.001, 3);
    let before = rms(&samples);
    let stats = agc_rms_16k(&mut samples);
    assert!(rms(&samples) <= before * 1.01);
    assert_eq!(stats.amplified_fraction, 0.0);
}

#[test]
fn speech_envelope_survives_level_alignment() {
    let mut samples = signal(0.0001, 1);
    for _ in 0..4 {
        samples.extend(signal(0.01, 1));
        samples.extend(signal(0.04, 1));
    }
    samples.extend(signal(0.0001, 1));
    agc_rms_16k(&mut samples);
    let quiet = rms(&samples[80_000..88_000]);
    let loud = rms(&samples[96_000..104_000]);
    assert!((3.6..4.4).contains(&(loud / quiet)), "{}", loud / quiet);
}

#[test]
fn quiet_onset_reaches_target_without_waiting_for_steady_state() {
    let mut samples = signal(0.0001, 1);
    samples.extend(signal(0.01, 4));
    samples.extend(signal(0.0001, 1));
    agc_rms_16k(&mut samples);
    let onset_rms = rms(&samples[16_000..19_200]);
    assert!(onset_rms >= 0.075, "first 200ms RMS: {onset_rms}");
}

#[test]
fn plan_apply_is_bitwise_identical_to_single_pass_agc() {
    let mut samples = signal(0.0001, 1);
    samples.extend(signal(0.01, 4));
    samples.extend(signal(0.0001, 1));
    let (plan, planned_stats) = agc_analysis_16k(&samples);
    let mut planned = samples.clone();
    agc_apply_16k(&mut planned, &plan);
    let mut direct = samples;
    let direct_stats = agc_rms_16k(&mut direct);
    assert_eq!(planned_stats.noise_floor, direct_stats.noise_floor);
    assert_eq!(planned_stats.speech_rms, direct_stats.speech_rms);
    assert_eq!(planned_stats.gain_p50, direct_stats.gain_p50);
    assert_eq!(planned_stats.gain_p95, direct_stats.gain_p95);
    assert_eq!(
        planned_stats.amplified_fraction,
        direct_stats.amplified_fraction
    );
    assert!(planned
        .iter()
        .zip(direct.iter())
        .all(|(a, b)| a.to_bits() == b.to_bits()));
}
