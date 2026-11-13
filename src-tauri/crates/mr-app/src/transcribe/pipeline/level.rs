use std::borrow::Cow;
use std::collections::HashMap;

pub(super) fn diarization_channels<'a>(
    source: &'a HashMap<usize, Vec<f32>>,
) -> HashMap<usize, Cow<'a, [f32]>> {
    source
        .iter()
        .map(|(&channel, samples)| {
            let (plan, stats) = mr_audio::dsp::agc_analysis_16k(samples);
            let prepared = if stats.amplified_fraction == 0.0 {
                Cow::Borrowed(samples.as_slice())
            } else {
                let mut amplified = samples.clone();
                mr_audio::dsp::agc_apply_16k(&mut amplified, &plan);
                Cow::Owned(amplified)
            };
            log::debug!(
                target: "diar",
                ch = channel as i64,
                noise_floor = stats.noise_floor,
                speech_rms = stats.speech_rms,
                gain_p50 = stats.gain_p50,
                gain_p95 = stats.gain_p95,
                amplified_fraction = stats.amplified_fraction;
                "ev=diar.agc"
            );
            (channel, prepared)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32, seconds: usize) -> Vec<f32> {
        (0..seconds * 16_000)
            .map(|index| {
                amplitude * (std::f32::consts::TAU * 440.0 * index as f32 / 16_000.0).sin()
            })
            .collect()
    }

    #[test]
    fn healthy_channel_without_amplification_preserves_original_samples() {
        let source = HashMap::from([(0usize, tone(0.5, 1))]);
        let prepared = diarization_channels(&source);
        assert!(matches!(prepared[&0], Cow::Borrowed(_)));
        assert!(prepared[&0]
            .iter()
            .zip(&source[&0])
            .all(|(actual, original)| actual.to_bits() == original.to_bits()));
    }

    #[test]
    fn quiet_speech_amplifies_into_owned_copy() {
        let mut samples = tone(0.0001, 1);
        samples.extend(tone(0.01, 4));
        samples.extend(tone(0.0001, 1));
        let source = HashMap::from([(0usize, samples)]);
        let prepared = diarization_channels(&source);
        let Cow::Owned(amplified) = &prepared[&0] else {
            panic!("тихая речь должна усиливаться в отдельную копию");
        };
        let before: f32 = source[&0][32_000..64_000]
            .iter()
            .map(|value| value.abs())
            .fold(0.0, f32::max);
        let after: f32 = amplified[32_000..64_000]
            .iter()
            .map(|value| value.abs())
            .fold(0.0, f32::max);
        assert!(after > before * 5.0, "before={before} after={after}");
        assert!(amplified[..16_000].iter().all(|value| value.abs() < 0.001));
    }

    #[test]
    fn plan_path_is_bitwise_identical_to_direct_agc() {
        let mut samples = tone(0.0001, 1);
        samples.extend(tone(0.01, 4));
        samples.extend(tone(0.0001, 1));
        let source = HashMap::from([(0usize, samples)]);
        let prepared = diarization_channels(&source);
        let mut direct = source[&0].clone();
        mr_audio::dsp::agc_rms_16k(&mut direct);
        assert!(prepared[&0]
            .iter()
            .zip(direct.iter())
            .all(|(a, b)| a.to_bits() == b.to_bits()));
    }
}
