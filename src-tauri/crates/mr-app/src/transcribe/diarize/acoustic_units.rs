use mr_ml::onnx::embed::EmbeddingExtractor;

use super::super::clean_speech::SpeechSupport;
use super::super::experiment::{self, Anchors, Embedding};
use super::{acoustic_windows, turn_anchors, AcoustUnit};

use crate::transcribe::echo::EchoEvidence;
use crate::transcribe::Utterance;

pub(super) struct Units {
    pub items: Vec<AcoustUnit>,
    pub anchors: Vec<usize>,
}

pub(crate) fn normalize_unit_rms(samples: &mut [f32]) {
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

struct PreparedUnit {
    utterance: usize,
    start: f64,
    end: f64,
    dur: f64,
    quality: f64,
    chunk: Vec<f32>,
    anchor: bool,
}

fn prepare(
    samples: &[f32],
    start: f64,
    end: f64,
    utterance: usize,
    support: SpeechSupport<'_>,
    anchor: bool,
) -> Option<PreparedUnit> {
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
    chunk.truncate(clean_len);
    let quality = super::rms_quality(&chunk);
    if experiment::current().embedding == Embedding::UnitNorm {
        normalize_unit_rms(&mut chunk);
    }
    Some(PreparedUnit {
        utterance,
        start,
        end,
        dur: clean_len as f64 / SR,
        quality,
        chunk,
        anchor,
    })
}

fn unit_from(prepared: PreparedUnit, embedding: Vec<f64>) -> AcoustUnit {
    AcoustUnit {
        utterance: prepared.utterance,
        start: prepared.start,
        end: prepared.end,
        dur: prepared.dur,
        quality: prepared.quality,
        embedding,
    }
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
        let mut prepared = Vec::new();
        if anchors_enabled {
            let region = match echo {
                Some(evidence) => {
                    super::local_region(evidence, start, end, super::MIN_LOCAL_EMBED_SEC)
                }
                None => Some((start, end)),
            };
            if let Some((start, end)) = region {
                let (start, end) = turn_anchors::support_bounds(start, end, support);
                if let Some(unit) = prepare(samples, start, end, index, support, true) {
                    prepared.push(unit);
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
            if let Some(unit) = prepare(samples, start, end, index, support, false) {
                prepared.push(unit);
            }
        }
        if !prepared.is_empty() {
            let started = std::time::Instant::now();
            let refs: Vec<&[f32]> = prepared.iter().map(|unit| unit.chunk.as_slice()).collect();
            let embeddings = match extractor.embed_batch(&refs) {
                Ok(embeddings) => embeddings,
                Err(error) => {
                    log::warn!(
                        target: "diar",
                        unit = index,
                        windows = prepared.len(),
                        error = error.to_string();
                        "ev=diar.embed_error"
                    );
                    prepared
                        .iter()
                        .map(|unit| extractor.embed(&unit.chunk).ok().flatten())
                        .collect()
                }
            };
            if log::log_enabled!(target: "diar", log::Level::Trace) {
                log::trace!(
                    target: "diar",
                    unit = index,
                    windows = prepared.len(),
                    elapsed_ms = started.elapsed().as_millis() as u64;
                    "ev=diar.embed_batch"
                );
            }
            for (prepared, embedding) in prepared.into_iter().zip(embeddings) {
                if let Some(embedding) = embedding {
                    if prepared.anchor {
                        units.anchors.push(units.items.len());
                    }
                    units.items.push(unit_from(prepared, embedding));
                }
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
