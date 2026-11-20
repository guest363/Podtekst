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

/// Короткие реплики (0.4–1.0 с) при строгом срезе VAD теряют атаку и хвост гласного,
/// что резко роняет сходство ERes2NetV2; 4 кадра (128 мс) в границах реплики без
/// перекрытий восстанавливают стабильность вектора без завышения чистой длительности.
const SHORT_CONTEXT_MAX_SEC: f64 = 1.0;
const SHORT_VAD_PAD_FRAMES: usize = 4;

pub(super) fn extract_clean_chunk(
    samples: &[f32],
    start: f64,
    end: f64,
    support: SpeechSupport<'_>,
) -> Option<(Vec<f32>, f64)> {
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
    let clean_dur = clean_len as f64 / SR;
    let final_len = if clean_dur < SHORT_CONTEXT_MAX_SEC && support.vad_probs.is_some() {
        chunk.copy_from_slice(&samples[lo..hi]);
        support.retain_with_vad_pad(&mut chunk, lo, SHORT_VAD_PAD_FRAMES)
    } else {
        clean_len
    };
    chunk.truncate(final_len);
    if experiment::current().embedding == Embedding::UnitNorm {
        normalize_unit_rms(&mut chunk);
    }
    Some((chunk, clean_dur))
}

fn prepare(
    samples: &[f32],
    start: f64,
    end: f64,
    utterance: usize,
    support: SpeechSupport<'_>,
    anchor: bool,
) -> Option<PreparedUnit> {
    let (chunk, clean_dur) = extract_clean_chunk(samples, start, end, support)?;
    let quality = super::rms_quality(&chunk);
    Some(PreparedUnit {
        utterance,
        start,
        end,
        dur: clean_dur,
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

fn ensure_anchor(units: &mut Units, utterance: usize) {
    if units
        .anchors
        .iter()
        .any(|&index| units.items[index].utterance == utterance)
    {
        return;
    }
    if let Some(index) = units
        .items
        .iter()
        .enumerate()
        .filter(|(_, unit)| unit.utterance == utterance)
        .max_by(|(_, a), (_, b)| a.dur.total_cmp(&b.dur))
        .map(|(index, _)| index)
    {
        units.anchors.push(index);
    }
}

/// Якорь и акустическое окно короткой реплики (≤2.4 с) — один и тот же срез:
/// второй инференс дал бы тот же вектор, поэтому он переиспользуется.
fn duplicate_sources(prepared: &[PreparedUnit]) -> Vec<Option<usize>> {
    let mut source = vec![None; prepared.len()];
    for (i, unit) in prepared.iter().enumerate().skip(1) {
        source[i] = (0..i).find(|&j| {
            prepared[j].start == unit.start
                && prepared[j].end == unit.end
                && prepared[j].chunk == unit.chunk
        });
    }
    source
}

/// Окна из нескольких реплик уходят в один батч: `embed_batch` группирует
/// только одинаковые длины, и батч на одну реплику почти всегда вырождается
/// в одиночные вызовы.
fn flush_batch(
    units: &mut Units,
    extractor: &mut EmbeddingExtractor,
    anchors_enabled: bool,
    prepared: &mut Vec<PreparedUnit>,
) {
    if prepared.is_empty() {
        return;
    }
    let started = std::time::Instant::now();
    let source = duplicate_sources(prepared);
    let unique: Vec<usize> = (0..prepared.len())
        .filter(|&index| source[index].is_none())
        .collect();
    let refs: Vec<&[f32]> = unique
        .iter()
        .map(|&index| prepared[index].chunk.as_slice())
        .collect();
    let mut embeddings = match extractor.embed_batch(&refs) {
        Ok(embeddings) => embeddings,
        Err(error) => {
            log::warn!(
                target: "diar",
                windows = unique.len(),
                error = error.to_string();
                "ev=diar.embed_error"
            );
            unique
                .iter()
                .map(|&index| extractor.embed(&prepared[index].chunk).ok().flatten())
                .collect()
        }
    };
    let windows = refs.len();
    drop(refs);
    let mut resolved: Vec<Option<Vec<f64>>> = vec![None; prepared.len()];
    for (slot, &index) in unique.iter().enumerate() {
        resolved[index] = embeddings[slot].take();
    }
    for (index, &source) in source.iter().enumerate() {
        if let Some(source) = source {
            resolved[index] = resolved[source].clone();
        }
    }
    let mut utterances: Vec<usize> = prepared.iter().map(|unit| unit.utterance).collect();
    utterances.sort_unstable();
    utterances.dedup();
    for (prepared, embedding) in prepared.drain(..).zip(resolved) {
        if let Some(embedding) = embedding {
            if prepared.anchor {
                units.anchors.push(units.items.len());
            }
            units.items.push(unit_from(prepared, embedding));
        }
    }
    if anchors_enabled {
        for utterance in utterances {
            ensure_anchor(units, utterance);
        }
    }
    if log::log_enabled!(target: "diar", log::Level::Trace) {
        log::trace!(
            target: "diar",
            windows = windows,
            elapsed_ms = started.elapsed().as_millis() as u64;
            "ev=diar.embed_batch"
        );
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
    const EMBED_PENDING: usize = 64;
    let mut pending: Vec<PreparedUnit> = Vec::new();
    for (index, utterance) in utterances.iter().enumerate() {
        let (start, end) = (utterance.start, utterance.end);
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
        pending.extend(prepared);
        if pending.len() >= EMBED_PENDING {
            flush_batch(&mut units, extractor, anchors_enabled, &mut pending);
        }
        if index % 20 == 0 || index + 1 == total {
            on_event(
                (80 + 8 * index / total.max(1)) as u8,
                &format!("Определяю спикеров: {index} из {total} реплик"),
            );
        }
    }
    flush_batch(&mut units, extractor, anchors_enabled, &mut pending);
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

    #[test]
    fn duplicate_anchor_and_window_chunk_is_embedded_once() {
        let unit = |start: f64, end: f64, chunk: Vec<f32>| PreparedUnit {
            utterance: 0,
            start,
            end,
            dur: end - start,
            quality: 1.0,
            chunk,
            anchor: false,
        };
        let prepared = vec![
            unit(0.0, 1.5, vec![0.1; 24_000]),
            unit(0.0, 1.5, vec![0.1; 24_000]),
            unit(0.0, 2.0, vec![0.1; 32_000]),
            unit(2.0, 4.0, vec![0.1; 32_000]),
        ];
        let source = duplicate_sources(&prepared);
        assert_eq!(source, vec![None, Some(0), None, None]);
    }

    #[test]
    fn anchor_falls_back_to_longest_unit_per_utterance() {
        let unit = |utterance: usize, dur: f64| AcoustUnit {
            utterance,
            start: 0.0,
            end: dur,
            dur,
            quality: 1.0,
            embedding: vec![],
        };
        let mut units = Units {
            items: vec![unit(0, 1.0), unit(0, 2.5), unit(1, 3.0)],
            anchors: vec![],
        };
        ensure_anchor(&mut units, 0);
        assert_eq!(units.anchors, vec![1]);
        units.anchors.push(2);
        ensure_anchor(&mut units, 1);
        assert_eq!(units.anchors, vec![1, 2]);
    }

    #[test]
    fn extract_clean_chunk_pads_only_short_chunks_and_drops_subminimum() {
        let mut vad = vec![0.0f32; 80];
        let samples = vec![0.1f32; vad.len() * mr_ml::onnx::vad::VAD_WINDOW];
        let frame_sec = mr_ml::onnx::vad::VAD_WINDOW as f64 / 16_000.0;

        for prob in &mut vad[10..20] {
            *prob = 1.0;
        }
        assert!(extract_clean_chunk(
            &samples,
            0.0,
            vad.len() as f64 * frame_sec,
            SpeechSupport {
                vad_probs: Some(&vad),
                segmentation: None,
            },
        )
        .is_none());

        for prob in &mut vad[20..26] {
            *prob = 1.0;
        }
        let (short_chunk, short_dur) = extract_clean_chunk(
            &samples,
            0.0,
            vad.len() as f64 * frame_sec,
            SpeechSupport {
                vad_probs: Some(&vad),
                segmentation: None,
            },
        )
        .unwrap();
        assert!((short_dur - 16.0 * frame_sec).abs() < 1e-6);
        assert_eq!(
            short_chunk.len(),
            (16 + 2 * SHORT_VAD_PAD_FRAMES) * mr_ml::onnx::vad::VAD_WINDOW
        );

        for prob in &mut vad[26..44] {
            *prob = 1.0;
        }
        let (long_chunk, long_dur) = extract_clean_chunk(
            &samples,
            0.0,
            vad.len() as f64 * frame_sec,
            SpeechSupport {
                vad_probs: Some(&vad),
                segmentation: None,
            },
        )
        .unwrap();
        assert!((long_dur - 34.0 * frame_sec).abs() < 1e-6);
        assert_eq!(long_chunk.len(), 34 * mr_ml::onnx::vad::VAD_WINDOW);
    }
}
