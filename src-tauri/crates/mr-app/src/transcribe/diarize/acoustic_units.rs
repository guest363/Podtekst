use super::super::clean_speech::SpeechSupport;
use super::super::experiment::{self, Anchors, Embedding};
use super::{
    acoustic_windows, turn_anchors, AcoustUnit, EchoEvidence, EmbeddingExtractor, Utterance,
};

pub(super) struct Units {
    pub items: Vec<AcoustUnit>,
    pub anchors: Vec<usize>,
}

fn normalize_unit_rms(samples: &mut [f32]) {
    let energy = samples.iter().map(|&x| x * x).sum::<f32>();
    let rms = (energy / samples.len() as f32).sqrt();
    if rms <= f32::EPSILON {
        return;
    }
    let gain = (mr_audio::dsp::TARGET_RMS / rms).min(mr_audio::dsp::MAX_GAIN);
    for sample in samples.iter_mut() {
        *sample *= gain;
    }
}

fn extract(
    samples: &[f32],
    start: f64,
    end: f64,
    utterance: usize,
    extractor: &mut EmbeddingExtractor,
    support: SpeechSupport<'_>,
    anchor: bool,
) -> Option<AcoustUnit> {
    const SR: f64 = 16000.0;
    let lo = ((start.max(0.0) * SR) as usize).min(samples.len());
    let hi = ((end.max(0.0) * SR) as usize).min(samples.len());
    if hi <= lo {
        return None;
    }
    let mut chunk = samples[lo..hi].to_vec();
    let clean_len = support.retain_in_place(&mut chunk, lo);
    if clean_len < (SR * acoustic_windows::MIN_UNIT_SEC) as usize {
        return None;
    }
    let clean = &mut chunk[..clean_len];
    let quality = super::rms_quality(clean);
    if experiment::current().embedding == Embedding::UnitNorm {
        normalize_unit_rms(clean);
    }
    let started = std::time::Instant::now();
    let result = extractor.embed(clean);
    if log::log_enabled!(target: "diar", log::Level::Trace) {
        let (outcome, dim, norm) = match &result {
            Ok(Some(embedding)) => (
                "ok",
                embedding.len(),
                embedding.iter().map(|x| x * x).sum::<f64>().sqrt(),
            ),
            Ok(None) => ("none", 0, 0.0),
            Err(_) => ("error", 0, 0.0),
        };
        log::trace!(
            target: "diar",
            unit = utterance,
            anchor = anchor,
            dur_sec = clean_len as f64 / SR,
            dim = dim,
            norm = norm,
            result = outcome,
            elapsed_ms = started.elapsed().as_millis() as u64;
            "ev=diar.embed"
        );
    }
    let embedding = result.ok().flatten()?;
    Some(AcoustUnit {
        utterance,
        start,
        end,
        dur: clean_len as f64 / SR,
        quality,
        embedding,
    })
}

pub(super) fn build(
    samples: &[f32],
    utterances: &[Utterance],
    extractor: &mut Option<EmbeddingExtractor>,
    support: SpeechSupport<'_>,
    echo: Option<&EchoEvidence>,
    on_event: &dyn Fn(u8, &str),
) -> Units {
    let mut units = Units {
        items: Vec::new(),
        anchors: Vec::new(),
    };
    let Some(extractor) = extractor else {
        return units;
    };
    let anchors_enabled = experiment::current().anchors == Anchors::Turns;
    let total = utterances.len();
    for (index, utterance) in utterances.iter().enumerate() {
        let (start, end) = (utterance.start, utterance.end);
        let first = units.items.len();
        let anchor_count = units.anchors.len();
        if anchors_enabled {
            let region = match echo {
                Some(evidence) => {
                    super::local_region(evidence, start, end, super::MIN_LOCAL_EMBED_SEC)
                }
                None => Some((start, end)),
            };
            if let Some((start, end)) = region {
                let (start, end) = turn_anchors::support_bounds(start, end, support);
                if let Some(unit) = extract(samples, start, end, index, extractor, support, true) {
                    units.anchors.push(units.items.len());
                    units.items.push(unit);
                }
            }
        }
        for (start, end) in acoustic_windows::bounds(start, end) {
            let region = match echo {
                Some(evidence) => {
                    super::local_region(evidence, start, end, super::MIN_LOCAL_EMBED_SEC)
                }
                None => Some((start, end)),
            };
            let Some((start, end)) = region else {
                continue;
            };
            if let Some(unit) = extract(samples, start, end, index, extractor, support, false) {
                units.items.push(unit);
            }
        }
        if anchors_enabled && units.anchors.len() == anchor_count {
            if let Some(index) = (first..units.items.len())
                .max_by(|&a, &b| units.items[a].dur.total_cmp(&units.items[b].dur))
            {
                units.anchors.push(index);
            }
        }
        if index % 20 == 0 || index + 1 == total {
            on_event(
                (80 + 8 * index / total.max(1)) as u8,
                &format!("Определяю спикеров: {index} из {total} реплик"),
            );
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn unitnorm_equalizes_loud_and_quiet_to_same_rms() {
        let speech: Vec<f32> = (0..16_000)
            .map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / 16_000.0).sin())
            .collect();
        let mut loud: Vec<f32> = speech.iter().map(|&x| x * 0.5).collect();
        let mut quiet: Vec<f32> = speech.iter().map(|&x| x * 0.005).collect();
        normalize_unit_rms(&mut loud);
        normalize_unit_rms(&mut quiet);
        let target = mr_audio::dsp::TARGET_RMS;
        assert!((rms(&loud) - target).abs() < 1e-5);
        assert!((rms(&quiet) - target).abs() < 1e-5);
        assert!((rms(&loud) - rms(&quiet)).abs() < 1e-5);
        let mut silence = vec![0.0f32; 16_000];
        normalize_unit_rms(&mut silence);
        assert!(silence.iter().all(|&x| x == 0.0));
    }
}
