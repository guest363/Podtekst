use super::super::clean_speech::SpeechSupport;
use super::super::experiment::{self, Anchors};
use super::{
    acoustic_windows, turn_anchors, AcoustUnit, EchoEvidence, EmbeddingExtractor, Utterance,
};

pub(super) struct Units {
    pub items: Vec<AcoustUnit>,
    pub anchors: Vec<usize>,
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
    let clean = &chunk[..clean_len];
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
        quality: super::rms_quality(clean),
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
