use super::super::Utterance;

pub(super) fn write(stage: &'static str, channel: usize, utterances: &[Utterance]) {
    if !cfg!(debug_assertions) || !crate::logging::dump_enabled() {
        return;
    }
    let segments: Vec<_> = utterances
        .iter()
        .map(|utterance| {
            serde_json::json!({
                "start": utterance.start,
                "end": utterance.end,
                "speaker": crate::logging::speaker_token(&utterance.speaker),
                "words": utterance.words.iter().map(|word| [word.start_sec, word.end_sec]).collect::<Vec<_>>(),
            })
        })
        .collect();
    crate::logging::write_artifact_json(
        &format!("diar/ch{channel}.{stage}.json"),
        &serde_json::json!({"schema": 1, "stage": stage, "channel": channel, "segments": segments}),
    );
}
