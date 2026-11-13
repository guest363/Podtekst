use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Instant;

use super::super::diag::{stage_begin, stage_end};
use super::super::diarize;
use super::super::echo::EchoEvidence;
use super::super::experiment::{self, Embedding};
use super::super::models::ModelSet;
use super::super::word_dedupe;
use super::super::word_speakers;
use super::super::{SpeakerInfo, TranscribeOptions, Utterance};

pub(super) struct Input<'a> {
    pub(super) channel_samples: &'a HashMap<usize, Vec<f32>>,
    pub(super) frontend_samples: &'a HashMap<usize, Cow<'a, [f32]>>,
    pub(super) chan_list: &'a [(usize, String)],
    pub(super) channel_vad: &'a HashMap<usize, Vec<f32>>,
    pub(super) channel_utts: HashMap<usize, Vec<Utterance>>,
    pub(super) echo: Option<EchoEvidence>,
    pub(super) models: &'a mut ModelSet,
    pub(super) opts: &'a TranscribeOptions,
    pub(super) is_stereo: bool,
    pub(super) on_event: &'a dyn Fn(u8, &str),
}

pub(super) fn run(input: Input<'_>) -> (Vec<SpeakerInfo>, Vec<Utterance>, Vec<String>) {
    let Input {
        channel_samples,
        frontend_samples,
        chan_list,
        channel_vad,
        mut channel_utts,
        echo,
        models,
        opts,
        is_stereo,
        on_event,
    } = input;
    let diar_stage = Instant::now();
    stage_begin("diar");
    models.ensure_diar();
    let profiles = diarize::load_profiles();
    let mut speakers_info: Vec<SpeakerInfo> = vec![];
    let mut all_utts: Vec<Utterance> = vec![];
    let mut channel_trees: HashMap<usize, word_speakers::DiarIntervalTree> = HashMap::new();
    let mut segmentations = HashMap::new();
    for (ch, _label) in chan_list.iter() {
        let samples: &[f32] = match experiment::current().embedding {
            Embedding::Frontend | Embedding::UnitNorm => &frontend_samples[ch][..],
            Embedding::Raw => &channel_samples[ch][..],
        };
        log::debug!(
            target: "diar",
            ch = *ch as i64,
            normalized = samples != channel_samples[ch].as_slice();
            "ev=diar.embedding_frontend"
        );
        let source_utts = channel_utts.remove(ch).unwrap_or_default();
        super::geometry::write("asr", *ch, &source_utts);
        let segmentation_started = Instant::now();
        let segmentation = if models.embed.is_some() && !source_utts.is_empty() {
            on_event(80, "Ищу одновременную речь…");
            models.segmentation.as_mut().and_then(|model| {
                let result = match channel_vad.get(ch) {
                    Some(probs) => model.segment_with_vad(&frontend_samples[ch], probs),
                    None => model.segment(&frontend_samples[ch]),
                };
                match result {
                    Ok(result) => {
                        let overlap_frames = result
                            .frames
                            .iter()
                            .filter(|&&class| class >= super::super::OVERLAP_CLASS)
                            .count();
                        log::debug!(
                            target: "diar",
                            ch = *ch as i64,
                            windows = result.windows.len(),
                            frames = result.frames.len(),
                            overlap_frames = overlap_frames,
                            elapsed_ms = segmentation_started.elapsed().as_millis() as u64;
                            "ev=diar.segmentation"
                        );
                        Some(result)
                    }
                    Err(error) => {
                        log::warn!(
                            target: "diar",
                            ch = *ch as i64,
                            reason = "segmentation_failed",
                            error = error.to_string();
                            "ev=diar.profile.load"
                        );
                        crate::note!("PyAnnote segmentation недоступна: {error}");
                        None
                    }
                }
            })
        } else {
            None
        };
        let paused_utts = diarize::split_by_word_pauses(&source_utts, 0.8);
        let mut diar_utts = if is_stereo {
            paused_utts
        } else {
            on_event(80, "Делю речь на паузы для спикеров…");
            diarize::split_at_pauses(&frontend_samples[ch], &paused_utts)
        };
        on_event(80, "Ищу смену голоса внутри реплик…");
        // Эхо бывает только в микрофонном канале (системный канал — источник).
        let ch_echo = if *ch == 0 { echo.as_ref() } else { None };
        diar_utts = diarize::split_long_by_speaker_change(
            &frontend_samples[ch],
            diar_utts,
            &mut models.embed,
            segmentation.as_ref(),
            ch_echo,
            &|pct, msg| on_event(pct, msg),
        );
        let (prefix, ch_idx) = if is_stereo {
            (
                if *ch == 0 {
                    "Участник"
                } else {
                    "Собеседник"
                },
                Some(*ch as i32),
            )
        } else {
            ("Собеседник", None)
        };
        let default_name = if *ch == 0 {
            opts.speaker_left.clone()
        } else {
            opts.speaker_right.clone()
        };
        super::geometry::write("pre-cluster", *ch, &diar_utts);
        let (infos, voice_runs) = diarize::diarize_channel(
            samples,
            &mut diar_utts,
            ch_idx,
            &default_name,
            prefix,
            &mut models.embed,
            segmentation.as_ref(),
            channel_vad.get(ch).map(Vec::as_slice),
            ch_echo,
            &profiles,
            opts.min_speakers,
            opts.max_speakers,
            &|pct, msg| on_event(pct, msg),
        );
        super::geometry::write("acoustic", *ch, &diar_utts);
        speakers_info.extend(infos);
        let tree = word_speakers::DiarIntervalTree::new(&diar_utts);
        let document = document_utterances(&source_utts, &tree);
        let document = word_speakers::detach_overlap_words(document, &voice_runs);
        if cfg!(debug_assertions) {
            let runs_dump: Vec<serde_json::Value> = voice_runs
                .iter()
                .map(|run| {
                    serde_json::json!({
                        "start": run.start,
                        "end": run.end,
                        "speaker": crate::logging::speaker_token(&run.speaker),
                    })
                })
                .collect();
            crate::logging::write_artifact_json(
                &format!("diar/ch{ch}.voice_runs.json"),
                &serde_json::json!({ "runs": runs_dump }),
            );
        }
        super::geometry::write("document", *ch, &document);
        all_utts.extend(document);
        channel_trees.insert(*ch, tree);
        if let Some(segmentation) = segmentation {
            segmentations.insert(*ch, segmentation);
        }
    }
    if let Some(evidence) = echo.as_ref() {
        let before = all_utts.len();
        let changed = word_dedupe::dedupe_words(
            &mut all_utts,
            channel_samples,
            evidence,
            channel_trees.get(&0),
        );
        log::debug!(
            target: "diar",
            changed = changed,
            before = before,
            after = all_utts.len();
            "ev=diar.word_dedupe"
        );
    }
    // Один и тот же голос в микрофоне и системе — одна карточка, а не два
    // разных «Участник N» / «Собеседник N». Дубли эха схлопываем до одной фразы.
    diarize::reconcile_channels(&mut speakers_info, &mut all_utts);
    diarize::dedupe_cross_channel(&mut all_utts, &speakers_info, channel_samples);
    all_utts.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.speaker.cmp(&b.speaker)));
    let mut merged = diarize::merge_utterances(all_utts);
    on_event(92, "Проверяю голоса отдельных реплик…");
    super::super::profile_verification::run(
        &mut merged,
        &mut speakers_info,
        channel_samples,
        &segmentations,
        &mut models.embed,
        &profiles,
    );
    stage_end("diar", diar_stage);
    diarize::refresh_speaker_stats(&mut speakers_info, &merged);
    if cfg!(debug_assertions) {
        let speaker_summary: Vec<serde_json::Value> = speakers_info
            .iter()
            .map(|info| {
                let mut value = serde_json::json!({
                    "id": crate::logging::speaker_token(&info.name),
                    "utts": info.utterances_count,
                    "speech_sec": info.total_speech_sec,
                    "silhouette": info.silhouette,
                    "similarity": info.similarity,
                    "matched": info.matched,
                    "candidates": info.candidates.iter().map(|(name, score)| {
                        serde_json::json!({
                            "name": crate::logging::speaker_token(name),
                            "similarity": score,
                        })
                    }).collect::<Vec<_>>(),
                });
                if crate::logging::dump_full() {
                    value["embedding"] = serde_json::json!(info.embedding);
                }
                value
            })
            .collect();
        crate::logging::write_artifact_json(
            "diar/summary.json",
            &serde_json::json!({"speakers": speaker_summary}),
        );
    }
    let speakers: Vec<String> = {
        let mut s = vec![];
        for u in merged.iter() {
            if !s.contains(&u.speaker) {
                s.push(u.speaker.clone());
            }
        }
        if s.is_empty() {
            s = speakers_info.iter().map(|x| x.name.clone()).collect();
        }
        s
    };
    (speakers_info, merged, speakers)
}

/// Пустые VAD-группы участвуют только в диаризации: в документ они не идут.
fn document_utterances(
    source_utts: &[Utterance],
    tree: &word_speakers::DiarIntervalTree,
) -> Vec<Utterance> {
    source_utts
        .iter()
        .filter(|utterance| !utterance.text.trim().is_empty())
        .flat_map(|utterance| word_speakers::split_utterance_by_word_speakers(utterance, tree))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utterance(start: f64, end: f64, text: &str) -> Utterance {
        Utterance {
            start,
            end,
            speaker: "Собеседник".to_string(),
            channel: Some(0),
            text: text.to_string(),
            diarized: false,
            silhouette: 0.0,
            words: Vec::new(),
            ctc_used: false,
        }
    }

    #[test]
    fn textless_vad_group_reaches_diarization_but_not_document() {
        let source = vec![utterance(0.0, 2.0, "раз два"), utterance(2.0, 5.0, "")];
        let samples = vec![0.1f32; 16000 * 6];
        let diar_utts = diarize::split_at_pauses(&samples, &source);
        assert!(diar_utts.iter().any(|u| u.text.is_empty()));
        let tree = word_speakers::DiarIntervalTree::new(&diar_utts);

        let doc_utts = document_utterances(&source, &tree);
        assert!(!doc_utts.is_empty());
        assert!(doc_utts.iter().all(|u| !u.text.is_empty()));
    }
}
