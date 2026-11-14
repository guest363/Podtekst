use std::collections::{BTreeMap, HashMap};

use super::super::clean_speech::SpeechSupport;
use super::{AcoustUnit, Utterance};

pub(super) fn core(
    stage: &'static str,
    ch: i32,
    units: &[AcoustUnit],
    core: &[usize],
    labels: &[usize],
    support: SpeechSupport<'_>,
) {
    clusters(
        stage,
        ch,
        core.iter().zip(labels).map(|(&index, &label)| {
            let unit = &units[index];
            (label, unit.start, unit.end)
        }),
        support,
    );
}

pub(super) fn turns(
    stage: &'static str,
    ch: i32,
    utterances: &[Utterance],
    labels: &HashMap<usize, usize>,
    support: SpeechSupport<'_>,
) {
    clusters(
        stage,
        ch,
        labels.iter().map(|(&index, &label)| {
            let utterance = &utterances[index];
            (label, utterance.start, utterance.end)
        }),
        support,
    );
}

fn clusters(
    stage: &'static str,
    ch: i32,
    intervals: impl IntoIterator<Item = (usize, f64, f64)>,
    support: SpeechSupport<'_>,
) {
    if !log::log_enabled!(target: "diar", log::Level::Debug) {
        return;
    }
    let mut grouped: BTreeMap<usize, Vec<(f64, f64)>> = BTreeMap::new();
    for (label, start, end) in intervals {
        grouped.entry(label).or_default().push((start, end));
    }
    let summary: Vec<_> = grouped
        .iter()
        .map(|(&label, spans)| {
            serde_json::json!({
                "label": label,
                "units": spans.len(),
                "speech_sec": support.duration(spans),
            })
        })
        .collect();
    log::debug!(
        target: "diar",
        ch = ch,
        stage = stage,
        k = summary.len(),
        clusters = serde_json::json!(summary).to_string();
        "ev=diar.clusters"
    );
}
