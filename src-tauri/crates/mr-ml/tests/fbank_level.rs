#![cfg(feature = "onnx")]

use mr_ml::onnx::features::eres2netv2_fbank;

#[test]
fn eres2netv2_features_are_stable_across_speech_levels() {
    let samples: Vec<f32> = (0..32_000)
        .map(|i| {
            let t = i as f32 / 16_000.0;
            let envelope = 0.08 + 0.92 * (std::f32::consts::PI * 2.1 * t).sin().abs();
            envelope
                * (0.12 * (std::f32::consts::TAU * 173.0 * t).sin()
                    + 0.06 * (std::f32::consts::TAU * 619.0 * t).sin()
                    + 0.025 * (std::f32::consts::TAU * 1927.0 * t).sin())
        })
        .collect();
    let reference = eres2netv2_fbank(&samples);
    for db in [-18.0_f32, -36.0] {
        let gain = 10.0_f32.powf(db / 20.0);
        let quiet: Vec<f32> = samples.iter().map(|sample| sample * gain).collect();
        let actual = eres2netv2_fbank(&quiet);
        assert_eq!(actual.len(), reference.len());
        let errors: Vec<f64> = actual
            .iter()
            .flatten()
            .zip(reference.iter().flatten())
            .map(|(&actual, &reference)| f64::from(actual - reference).powi(2))
            .collect();
        let rms = (errors.iter().sum::<f64>() / errors.len() as f64).sqrt();
        assert!(rms < 0.05, "{db} dB: RMS {rms}");
    }
}
