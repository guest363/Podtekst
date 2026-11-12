use std::collections::HashMap;

pub(super) fn diarization_channels(source: &HashMap<usize, Vec<f32>>) -> HashMap<usize, Vec<f32>> {
    source
        .iter()
        .map(|(&channel, samples)| {
            let mut prepared = samples.clone();
            let stats = mr_audio::dsp::agc_rms_16k(&mut prepared);
            if stats.amplified_fraction == 0.0 {
                prepared.copy_from_slice(samples);
            }
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

    #[test]
    fn healthy_channel_without_amplification_preserves_original_samples() {
        let samples: Vec<f32> = (0..16000)
            .map(|index| 0.5 * (std::f32::consts::TAU * 440.0 * index as f32 / 16000.0).sin())
            .collect();
        let source = HashMap::from([(0, samples)]);
        let prepared = diarization_channels(&source);
        assert!(prepared[&0]
            .iter()
            .zip(&source[&0])
            .all(|(actual, original)| actual.to_bits() == original.to_bits()));
    }
}
