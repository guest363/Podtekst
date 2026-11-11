use std::collections::{HashMap, HashSet};

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;
use mr_ml::{
    match_voiceprints_matrix, score_speaker_similarity, weighted_centroid, SpeakerProfile,
};

use super::diarize::pick_samples;
use super::{SpeakerInfo, Utterance};

/// Калибровка ERes2NetV2: `onnx_real` проверяет различимость голосов на фрагментах от 1,5 с.
const MIN_VERIFY_SEC: f64 = 1.5;

fn contradicts(embedding: &[f64], assigned: &str, profiles: &[SpeakerProfile]) -> bool {
    let Some(profile) = profiles.iter().find(|profile| profile.name == assigned) else {
        return false;
    };
    let own = score_speaker_similarity(embedding, profile);
    let best = profiles
        .iter()
        .map(|profile| score_speaker_similarity(embedding, profile))
        .fold(own, f64::max);
    own < mr_ml::DEFAULT_MATCH_THRESHOLD || best - own >= mr_ml::DEFAULT_MIN_MARGIN
}

pub(super) fn run(
    utterances: &mut Vec<Utterance>,
    infos: &mut Vec<SpeakerInfo>,
    channels: &HashMap<usize, Vec<f32>>,
    segmentations: &HashMap<usize, Segmentation>,
    extractor: &mut Option<EmbeddingExtractor>,
    profiles: &[SpeakerProfile],
) {
    let Some(extractor) = extractor else {
        return;
    };
    let mut names: HashSet<String> = infos.iter().map(|info| info.name.clone()).collect();
    let mut verified: HashMap<String, Vec<(Vec<f64>, f64)>> = HashMap::new();
    let mut changed = HashSet::new();
    let mut next_unknown = 1;
    let mut reassigned = 0;
    let mut failures = 0;
    for utterance in utterances.iter_mut() {
        let Some(info) = infos
            .iter()
            .find(|info| info.name == utterance.speaker && info.matched)
        else {
            continue;
        };
        let Some(channel) = utterance
            .channel
            .and_then(|channel| usize::try_from(channel).ok())
        else {
            continue;
        };
        let Some(samples) = channels.get(&channel) else {
            continue;
        };
        let start = utterance.start;
        let end = utterance.end;
        let lo = ((start.max(0.0) * 16000.0) as usize).min(samples.len());
        let hi = ((end.max(start).max(0.0) * 16000.0) as usize).min(samples.len());
        if hi.saturating_sub(lo) < (MIN_VERIFY_SEC * 16000.0) as usize {
            continue;
        }
        let mut chunk = samples[lo..hi].to_vec();
        if let Some(segmentation) = segmentations.get(&channel) {
            let clean = segmentation.exclude_overlap(&mut chunk, lo);
            chunk.truncate(clean);
        }
        let duration = chunk.len() as f64 / 16000.0;
        if duration < MIN_VERIFY_SEC {
            continue;
        }
        let embedding = match extractor.embed(&chunk) {
            Ok(Some(embedding)) => embedding,
            _ => {
                failures += 1;
                continue;
            }
        };
        if contradicts(&embedding, &utterance.speaker, profiles) {
            let mut result = match_voiceprints_matrix(
                std::slice::from_ref(&embedding),
                profiles,
                mr_ml::DEFAULT_MATCH_THRESHOLD,
                mr_ml::DEFAULT_MIN_MARGIN,
            );
            let result = result.remove(0);
            let name = result.name.clone().unwrap_or_else(|| {
                while names.contains(&format!("Собеседник {next_unknown}")) {
                    next_unknown += 1;
                }
                let name = format!("Собеседник {next_unknown}");
                next_unknown += 1;
                name
            });
            if !names.contains(&name) {
                let mut new_info = info.clone();
                new_info.id = format!("verified_{}", infos.len());
                new_info.name = name.clone();
                new_info.channel = utterance.channel;
                new_info.matched = result.matched;
                new_info.matched_name = result.name;
                new_info.matched_pack_id = result.pack_id;
                new_info.matched_pack_name = result.pack_name;
                new_info.similarity = result.similarity;
                new_info.silhouette = 0.0;
                new_info.candidates = result
                    .candidates
                    .into_iter()
                    .map(|candidate| (candidate.name, candidate.similarity))
                    .collect();
                new_info.embedding = None;
                new_info.samples.clear();
                infos.push(new_info);
                names.insert(name.clone());
            }
            changed.insert(utterance.speaker.clone());
            utterance.speaker = name;
            utterance.silhouette = 0.0;
            reassigned += 1;
        }
        verified
            .entry(utterance.speaker.clone())
            .or_default()
            .push((embedding, duration));
    }
    for info in infos.iter_mut() {
        if let Some(points) = verified.remove(&info.name) {
            let (vectors, weights): (Vec<_>, Vec<_>) = points.into_iter().unzip();
            let embedding = weighted_centroid(&vectors, &weights);
            if let Some(profile) = profiles
                .iter()
                .find(|profile| info.matched && profile.name == info.name)
            {
                info.similarity =
                    (score_speaker_similarity(&embedding, profile) * 1000.0).round() / 1000.0;
            }
            let mut result = match_voiceprints_matrix(
                std::slice::from_ref(&embedding),
                profiles,
                mr_ml::DEFAULT_MATCH_THRESHOLD,
                mr_ml::DEFAULT_MIN_MARGIN,
            );
            info.candidates = result
                .remove(0)
                .candidates
                .into_iter()
                .map(|candidate| (candidate.name, candidate.similarity))
                .collect();
            info.embedding = Some(embedding);
        } else if changed.contains(&info.name) {
            info.embedding = None;
            info.similarity = 0.0;
            info.candidates.clear();
        }
    }
    super::diarize::dedupe_cross_channel(utterances, infos, channels);
    for info in infos.iter_mut() {
        let members: Vec<_> = utterances
            .iter()
            .filter(|utterance| utterance.speaker == info.name)
            .cloned()
            .collect();
        info.samples = pick_samples(&members, 3);
    }
    log::debug!(target: "diar", reassigned = reassigned, failures = failures; "ev=diar.verify_profiles");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, score: f64) -> SpeakerProfile {
        SpeakerProfile {
            name: name.to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
            embedding: vec![score, (1.0 - score * score).sqrt()],
            samples_count: 1,
            updated_at: String::new(),
            prototypes: vec![],
        }
    }

    #[test]
    fn isolated_voice_can_contradict_its_named_cluster() {
        let profiles = vec![profile("A", 0.5), profile("B", 0.8)];
        assert!(contradicts(&[1.0, 0.0], "A", &profiles));
        assert!(!contradicts(&[1.0, 0.0], "B", &profiles));
    }

    #[test]
    fn close_scores_and_missing_profiles_do_not_force_relabeling() {
        let profiles = vec![profile("A", 0.70), profile("B", 0.71)];
        assert!(!contradicts(&[1.0, 0.0], "A", &profiles));
        assert!(!contradicts(&[1.0, 0.0], "C", &profiles));
    }

    #[test]
    fn unsupported_cluster_identity_is_rejected() {
        assert!(contradicts(&[1.0, 0.0], "A", &[profile("A", 0.2)]));
    }
    #[test]
    fn ambiguous_alternative_is_not_forced_into_a_profile() {
        let profiles = vec![
            profile("A", 0.45),
            profile("B", 0.55),
            profile("C", 0.53),
            profile("D", 0.51),
            profile("E", 0.49),
        ];
        let query = vec![1.0, 0.0];
        assert!(contradicts(&query, "A", &profiles));
        let result = match_voiceprints_matrix(
            &[query],
            &profiles,
            mr_ml::DEFAULT_MATCH_THRESHOLD,
            mr_ml::DEFAULT_MIN_MARGIN,
        );
        assert!(!result[0].matched);
        assert!(result[0].name.is_none());
    }

    #[test]
    fn real_voice_relabels_utterance_and_rebuilds_its_sample() {
        let Some(models) = std::env::var_os("MR_MODELS_DIR") else {
            return;
        };
        let model =
            std::path::PathBuf::from(models).join("diar-eres2netv2-emb/eres2netv2_emb.onnx");
        let mut extractor = EmbeddingExtractor::load(&model).expect("модель ERes2NetV2");
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mr-ml/tests/fixtures");
        let load = |name: &str| {
            hound::WavReader::open(fixtures.join(name))
                .expect("открытый WAV")
                .samples::<i16>()
                .map(|sample| sample.expect("PCM") as f32 / 32768.0)
                .collect::<Vec<_>>()
        };
        let samples = load("speaker1_a_cn_16k.wav");
        let other = load("speaker2_a_cn_16k.wav");
        let own = extractor
            .embed(&samples)
            .expect("инференс")
            .expect("голос A");
        let alien = extractor.embed(&other).expect("инференс").expect("голос B");
        let mut a = profile("A", 0.5);
        a.embedding = own;
        let mut b = profile("B", 0.5);
        b.embedding = alien;
        let profiles = vec![a, b];
        let duration = samples.len() as f64 / 16000.0;
        let mut utterances = vec![Utterance {
            start: 0.0,
            end: duration,
            speaker: "B".to_string(),
            channel: Some(1),
            text: "раз два".to_string(),
            diarized: true,
            silhouette: 0.7,
            words: vec![],
            ctc_used: false,
        }];
        let mut infos = vec![SpeakerInfo {
            id: "B".to_string(),
            name: "B".to_string(),
            channel: Some(1),
            matched: true,
            matched_name: Some("B".to_string()),
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: 0.99,
            silhouette: 0.7,
            candidates: vec![],
            embedding: None,
            samples: vec![],
            utterances_count: 1,
            total_speech_sec: duration,
        }];
        let mut echo = utterances[0].clone();
        echo.channel = Some(0);
        utterances.insert(0, echo);
        let quiet: Vec<f32> = samples.iter().map(|sample| sample * 0.35).collect();
        run(
            &mut utterances,
            &mut infos,
            &HashMap::from([(0, quiet), (1, samples)]),
            &HashMap::new(),
            &mut Some(extractor),
            &profiles,
        );
        super::super::diarize::refresh_speaker_stats(&mut infos, &utterances);
        assert_eq!(utterances.len(), 1);
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(utterances[0].text, "раз два");
        assert_eq!(utterances[0].end, duration);
        assert_eq!(utterances[0].channel, Some(1));
        assert_eq!(infos.len(), 1);
        assert!(infos[0].matched);
        assert_eq!(infos[0].samples[0].channel, Some(1));
        assert!((infos[0].similarity - 1.0).abs() < 0.001);
    }
}
