use std::collections::{HashMap, HashSet};

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;
use mr_ml::{
    cosine_similarity, match_voiceprints_matrix, score_speaker_similarity, weighted_centroid,
    SpeakerProfile,
};

use super::clean_speech::SpeechSupport;
use super::diarize::acoustic_units::normalize_unit_rms;
use super::diarize::{local_region, pick_samples, MIN_LOCAL_EMBED_SEC};
use super::echo::EchoEvidence;
use super::experiment::{self, Embedding};
use super::{SpeakerInfo, Utterance};

/// Калибровка ERes2NetV2: `onnx_real` проверяет различимость голосов на фрагментах от 1,5 с.
const MIN_VERIFY_SEC: f64 = 1.5;
/// Та же граница «один голос», что в `reconcile_channels`; иначе каждая спорная реплика плодит карточку.
const ANON_MERGE_SIM: f64 = 0.55;
/// Замер живой записи: короткое «угу» 0.91 с зажато соседями того же канала
/// (паузы 1.18/1.55 с на акустической стадии); пословная атрибуция сдвигает
/// границу следующей реплики до 2.08 с, поэтому допуск 2.2 с.
const ADOPT_PAUSE_SEC: f64 = 2.2;
/// Замер ERes2NetV2 на фронтенд-канале живой записи: соседние куски одного
/// голоса с паузами 0.72–0.78 с — cos 0.6267–0.7606, реплики разных голосов — ≤ 0.378.
const CHAIN_PAUSE_SEC: f64 = 1.5;
const CHAIN_MIN_SIM: f64 = 0.5;
/// Замер живой записи: ложный переворот реплики 1.7 с — маржа 0.0289, настоящие
/// перевороты — маржа 0.30+; на коротких кусках переворачивает только уверенная смена.
const SHORT_FLIP_SEC: f64 = 3.0;
const SHORT_FLIP_MARGIN: f64 = 0.06;

struct ChannelNeighbor {
    speaker: String,
    end: f64,
    embedding: Vec<f64>,
    matched: bool,
}

fn nearest_anonymous<'a>(
    infos: &'a [SpeakerInfo],
    embedding: &[f64],
    current: &str,
) -> Option<&'a str> {
    let mut scored: Vec<(f64, &str)> = infos
        .iter()
        .filter(|info| !info.matched && info.name != current)
        .filter_map(|info| {
            info.embedding
                .as_ref()
                .map(|vector| (cosine_similarity(vector, embedding), info.name.as_str()))
        })
        .filter(|(score, _)| *score >= ANON_MERGE_SIM)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (score, name) = *scored.first()?;
    // Как в общем матчере: близкий второй кандидат — неоднозначность, а не выбор.
    if scored
        .get(1)
        .is_some_and(|second| score - second.0 < mr_ml::DEFAULT_MIN_MARGIN)
    {
        return None;
    }
    Some(name)
}

fn adopt_short_between_matched_neighbors(
    utterances: &mut [Utterance],
    infos: &[SpeakerInfo],
    changed: &HashSet<String>,
) -> usize {
    let matched: HashSet<&str> = infos
        .iter()
        .filter(|info| info.matched)
        .map(|info| info.name.as_str())
        .collect();
    let mut adopted = 0;
    for index in 0..utterances.len() {
        let (start, end, channel) = {
            let current = &utterances[index];
            (current.start, current.end, current.channel)
        };
        if end - start >= MIN_VERIFY_SEC {
            continue;
        }
        let own = utterances[index].speaker.clone();
        let prev = utterances[..index]
            .iter()
            .rev()
            .find(|u| u.channel == channel);
        let next = utterances[index + 1..]
            .iter()
            .find(|u| u.channel == channel);
        let Some(target) = prev
            .zip(next)
            .filter(|(prev, next)| {
                prev.speaker == next.speaker
                    && start - prev.end <= ADOPT_PAUSE_SEC
                    && next.start - end <= ADOPT_PAUSE_SEC
                    && matched.contains(prev.speaker.as_str())
            })
            .map(|(prev, _)| prev.speaker.clone())
        else {
            continue;
        };
        if own == target {
            continue;
        }
        // Устойчивая собственная matched-карточка — акустическое свидетельство кластера;
        // соседи перетягивают только карточку-микс, покинутую проверенными голосами.
        if matched.contains(own.as_str()) && !changed.contains(&own) {
            continue;
        }
        utterances[index].speaker = target;
        utterances[index].silhouette = 0.0;
        adopted += 1;
    }
    adopted
}

fn chain_inherit(prev: &ChannelNeighbor, utterance: &Utterance, embedding: &[f64]) -> bool {
    prev.matched
        && utterance.start - prev.end <= CHAIN_PAUSE_SEC
        && cosine_similarity(&prev.embedding, embedding) >= CHAIN_MIN_SIM
}

/// Наследование — свидетельство соседа; у реплики со своей подтверждённой
/// карточкой оно сильнее собственного замера и схлопывает двух людей в одного.
fn inherit_allowed(
    verify_card: bool,
    prev: Option<&ChannelNeighbor>,
    utterance: &Utterance,
    embedding: &[f64],
) -> bool {
    !verify_card && prev.is_some_and(|prev| chain_inherit(prev, utterance, embedding))
}

/// Сигнал и маска для верификации — те же, что у диаризации: иначе «сырое»
/// аудио канала систематически расходится с центроидами и имя теряется.
/// Имя профиля требует 2 с чистого аудио (`MIN_NAMED_CLEAN_SEC`), а
/// верификация режет чанк от 1.5 с — на коротком куске имя не присваиваем.
fn may_claim_name(duration: f64) -> bool {
    duration >= super::diarize::MIN_NAMED_CLEAN_SEC
}

pub(super) struct VerificationInput<'a> {
    pub(super) samples: &'a HashMap<usize, &'a [f32]>,
    pub(super) vad: &'a HashMap<usize, Vec<f32>>,
    pub(super) segmentations: &'a HashMap<usize, Segmentation>,
    pub(super) echo: Option<&'a EchoEvidence>,
}

fn verification_chunk(
    samples: &[f32],
    support: SpeechSupport<'_>,
    echo: Option<&EchoEvidence>,
    start: f64,
    end: f64,
) -> Option<Vec<f32>> {
    const SR: f64 = 16000.0;
    let (start, end) = match echo {
        // Кадры «только эхо» — голос источника, а не владельца карточки:
        // без локальной речи верифицировать нечем, имя остаётся как есть.
        Some(evidence) => local_region(evidence, start, end, MIN_LOCAL_EMBED_SEC)?,
        None => (start, end),
    };
    let lo = ((start.max(0.0) * SR) as usize).min(samples.len());
    let hi = ((end.max(0.0) * SR) as usize).min(samples.len());
    if hi.saturating_sub(lo) < (MIN_VERIFY_SEC * SR) as usize {
        return None;
    }
    let mut chunk = samples[lo..hi].to_vec();
    let clean = support.retain_in_place(&mut chunk, lo);
    chunk.truncate(clean);
    if experiment::current().embedding == Embedding::UnitNorm {
        normalize_unit_rms(&mut chunk);
    }
    Some(chunk)
}

fn contradicts(
    embedding: &[f64],
    assigned: &str,
    profiles: &[SpeakerProfile],
    flip_margin: f64,
) -> bool {
    let Some(profile) = profiles.iter().find(|profile| profile.name == assigned) else {
        return false;
    };
    let own = score_speaker_similarity(embedding, profile);
    let best = profiles
        .iter()
        .map(|profile| score_speaker_similarity(embedding, profile))
        .fold(own, f64::max);
    own < mr_ml::DEFAULT_MATCH_THRESHOLD || best - own >= flip_margin
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    utterances: &mut Vec<Utterance>,
    infos: &mut Vec<SpeakerInfo>,
    channels: &HashMap<usize, Vec<f32>>,
    audio: VerificationInput<'_>,
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
    let mut chained = 0;
    let mut failures = 0;
    let mut last_on_channel: HashMap<usize, ChannelNeighbor> = HashMap::new();
    for utterance in utterances.iter_mut() {
        let Some(channel) = utterance
            .channel
            .and_then(|channel| usize::try_from(channel).ok())
        else {
            continue;
        };
        let chain_prev = last_on_channel
            .get(&channel)
            .filter(|prev| prev.matched && utterance.start - prev.end <= CHAIN_PAUSE_SEC);
        let verify_card = infos
            .iter()
            .any(|info| info.name == utterance.speaker && info.matched);
        if chain_prev.is_none() && !verify_card {
            continue;
        }
        let Some(samples) = audio.samples.get(&channel) else {
            continue;
        };
        let support = SpeechSupport {
            vad_probs: audio.vad.get(&channel).map(Vec::as_slice),
            segmentation: audio.segmentations.get(&channel),
        };
        let Some(chunk) =
            verification_chunk(samples, support, audio.echo, utterance.start, utterance.end)
        else {
            continue;
        };
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
        let inherited = inherit_allowed(verify_card, chain_prev, utterance, &embedding);
        if inherited {
            if let Some(prev) = chain_prev {
                if utterance.speaker != prev.speaker {
                    changed.insert(std::mem::replace(
                        &mut utterance.speaker,
                        prev.speaker.clone(),
                    ));
                    utterance.silhouette = 0.0;
                    chained += 1;
                }
            }
        } else if verify_card {
            let flip_margin = if duration < SHORT_FLIP_SEC {
                SHORT_FLIP_MARGIN
            } else {
                mr_ml::DEFAULT_MIN_MARGIN
            };
            if contradicts(&embedding, &utterance.speaker, profiles, flip_margin) {
                let mut result = match_voiceprints_matrix(
                    std::slice::from_ref(&embedding),
                    profiles,
                    mr_ml::DEFAULT_MATCH_THRESHOLD,
                    mr_ml::DEFAULT_MIN_MARGIN,
                );
                let result = result.remove(0);
                // Имя профиля требует 2 с чистого аудио: на 1.5-секундном куске
                // карточка остаётся анонимной, даже если матчер уверен.
                let named = may_claim_name(duration);
                let name = if named { result.name.clone() } else { None }.unwrap_or_else(|| {
                    if let Some(known) = nearest_anonymous(infos, &embedding, &utterance.speaker) {
                        return known.to_string();
                    }
                    while names.contains(&format!("Собеседник {next_unknown}")) {
                        next_unknown += 1;
                    }
                    let name = format!("Собеседник {next_unknown}");
                    next_unknown += 1;
                    name
                });
                if !names.contains(&name) {
                    let mut new_info = infos
                        .iter()
                        .find(|info| info.name == utterance.speaker && info.matched)
                        .cloned()
                        .expect("verify_card");
                    new_info.id = format!("verified_{}", infos.len());
                    new_info.name = name.clone();
                    new_info.channel = utterance.channel;
                    new_info.matched = named && result.matched;
                    new_info.matched_name = new_info.matched.then(|| result.name.clone()).flatten();
                    new_info.matched_pack_id =
                        new_info.matched.then(|| result.pack_id.clone()).flatten();
                    new_info.matched_pack_name =
                        new_info.matched.then(|| result.pack_name.clone()).flatten();
                    new_info.similarity = if new_info.matched {
                        result.similarity
                    } else {
                        0.0
                    };
                    new_info.silhouette = 0.0;
                    new_info.candidates = result
                        .candidates
                        .into_iter()
                        .map(|candidate| (candidate.name, candidate.similarity))
                        .collect();
                    new_info.embedding = Some(embedding.clone());
                    new_info.samples.clear();
                    infos.push(new_info);
                    names.insert(name.clone());
                }
                changed.insert(utterance.speaker.clone());
                utterance.speaker = name;
                utterance.silhouette = 0.0;
                reassigned += 1;
            }
        }
        if inherited || verify_card {
            verified
                .entry(utterance.speaker.clone())
                .or_default()
                .push((embedding.clone(), duration));
        }
        let matched = infos
            .iter()
            .any(|info| info.name == utterance.speaker && info.matched);
        last_on_channel.insert(
            channel,
            ChannelNeighbor {
                speaker: utterance.speaker.clone(),
                end: utterance.end,
                embedding,
                matched,
            },
        );
    }
    let adopted = adopt_short_between_matched_neighbors(utterances, infos, &changed);
    for info in infos.iter_mut() {
        if let Some(points) = verified.remove(&info.name) {
            let (vectors, weights): (Vec<_>, Vec<_>) = points.into_iter().unzip();
            let embedding = weighted_centroid(&vectors, &weights);
            if info.matched {
                if let Some(profile) = profiles.iter().find(|profile| profile.name == info.name) {
                    info.similarity =
                        (score_speaker_similarity(&embedding, profile) * 1000.0).round() / 1000.0;
                }
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
            // Карточка потеряла свой эмбеддинг: «узнан · 0.00» без вектора — ложное состояние.
            info.embedding = None;
            info.similarity = 0.0;
            info.candidates.clear();
            info.matched = false;
            info.matched_name = None;
            info.matched_pack_id = None;
            info.matched_pack_name = None;
        }
    }
    super::diarize::dedupe_cross_channel(utterances, infos, channels);
    for info in infos.iter_mut() {
        let members: Vec<&Utterance> = utterances
            .iter()
            .filter(|utterance| utterance.speaker == info.name)
            .collect();
        info.samples = pick_samples(&members, 3);
    }
    log::debug!(
        target: "diar",
        reassigned = reassigned,
        chained = chained,
        adopted = adopted,
        failures = failures;
        "ev=diar.verify_profiles"
    );
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
        assert!(contradicts(
            &[1.0, 0.0],
            "A",
            &profiles,
            mr_ml::DEFAULT_MIN_MARGIN
        ));
        assert!(!contradicts(
            &[1.0, 0.0],
            "B",
            &profiles,
            mr_ml::DEFAULT_MIN_MARGIN
        ));
    }

    #[test]
    fn close_scores_and_missing_profiles_do_not_force_relabeling() {
        let profiles = vec![profile("A", 0.70), profile("B", 0.71)];
        assert!(!contradicts(
            &[1.0, 0.0],
            "A",
            &profiles,
            mr_ml::DEFAULT_MIN_MARGIN
        ));
        assert!(!contradicts(
            &[1.0, 0.0],
            "C",
            &profiles,
            mr_ml::DEFAULT_MIN_MARGIN
        ));
    }

    #[test]
    fn unsupported_cluster_identity_is_rejected() {
        assert!(contradicts(
            &[1.0, 0.0],
            "A",
            &[profile("A", 0.2)],
            mr_ml::DEFAULT_MIN_MARGIN
        ));
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
        assert!(contradicts(
            &query,
            "A",
            &profiles,
            mr_ml::DEFAULT_MIN_MARGIN
        ));
        let result = match_voiceprints_matrix(
            &[query],
            &profiles,
            mr_ml::DEFAULT_MATCH_THRESHOLD,
            mr_ml::DEFAULT_MIN_MARGIN,
        );
        assert!(!result[0].matched);
        assert!(result[0].name.is_none());
    }

    fn info(name: &str, matched: bool, embedding: Vec<f64>) -> SpeakerInfo {
        SpeakerInfo {
            id: name.to_string(),
            name: name.to_string(),
            channel: Some(0),
            matched,
            matched_name: None,
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: 0.0,
            silhouette: 0.0,
            candidates: vec![],
            embedding: Some(embedding),
            samples: vec![],
            utterances_count: 1,
            total_speech_sec: 1.0,
        }
    }

    #[test]
    fn anonymous_voice_is_reused_instead_of_a_new_card() {
        let infos = vec![
            info("Собеседник 1", false, vec![1.0, 0.0, 0.0]),
            info("A", true, vec![1.0, 0.0, 0.0]),
            info("Другой", false, vec![0.0, 1.0, 0.0]),
        ];
        assert_eq!(
            nearest_anonymous(&infos, &[1.0, 0.0, 0.0], "Исходный"),
            Some("Собеседник 1")
        );
        assert_eq!(
            nearest_anonymous(&infos, &[0.0, 0.0, 1.0], "Исходный"),
            None
        );
        assert_eq!(
            nearest_anonymous(&infos, &[1.0, 0.0, 0.0], "Собеседник 1"),
            None
        );
    }

    fn anon_at(name: &str, similarity: f64) -> SpeakerInfo {
        let angle = similarity.clamp(-1.0, 1.0).acos();
        info(name, false, vec![angle.cos(), angle.sin(), 0.0])
    }

    #[test]
    fn ambiguous_anonymous_candidates_are_not_merged() {
        let infos = vec![anon_at("Собеседник 1", 0.57), anon_at("Собеседник 2", 0.56)];
        assert_eq!(
            nearest_anonymous(&infos, &[1.0, 0.0, 0.0], "Исходный"),
            None,
            "маржа меньше DEFAULT_MIN_MARGIN — выбор неоднозначен"
        );
        let clear = vec![anon_at("Собеседник 1", 0.9), anon_at("Собеседник 2", 0.55)];
        assert_eq!(
            nearest_anonymous(&clear, &[1.0, 0.0, 0.0], "Исходный"),
            Some("Собеседник 1")
        );
    }

    #[test]
    fn short_fragment_cannot_claim_a_profile_name() {
        assert!(!may_claim_name(MIN_VERIFY_SEC));
        assert!(may_claim_name(2.0));
    }

    fn utterance(start: f64, end: f64, speaker: &str) -> Utterance {
        Utterance {
            start,
            end,
            speaker: speaker.to_string(),
            channel: Some(0),
            text: String::new(),
            diarized: true,
            silhouette: 0.5,
            words: vec![],
            ctc_used: false,
        }
    }

    fn neighbor(speaker: &str, end: f64, matched: bool) -> ChannelNeighbor {
        ChannelNeighbor {
            speaker: speaker.to_string(),
            end,
            embedding: vec![1.0, 0.0],
            matched,
        }
    }

    #[test]
    fn own_matched_card_is_not_overridden_by_neighbor_chain() {
        let prev = neighbor("A", 10.0, true);
        let current = utterance(11.0, 13.0, "B");
        assert!(chain_inherit(&prev, &current, &[1.0, 0.0]));
        assert!(
            !inherit_allowed(true, Some(&prev), &current, &[1.0, 0.0]),
            "своя подтверждённая карточка сильнее соседа"
        );
        assert!(inherit_allowed(false, Some(&prev), &current, &[1.0, 0.0]));
    }

    fn echo_evidence(frames: Vec<crate::transcribe::echo::FrameClass>) -> EchoEvidence {
        EchoEvidence {
            delay_samples: 1000,
            delay_sec: 0.0625,
            prominence: 12.0,
            stability: 1.0,
            gain: 0.1,
            noise_floor: 1e-4,
            residual: vec![0.0; frames.len()],
            frames,
        }
    }

    #[test]
    fn echo_only_region_is_not_verified() {
        use crate::transcribe::echo::FrameClass;
        let win = mr_ml::onnx::vad::VAD_WINDOW as f64 / 16000.0;
        let evidence = echo_evidence(vec![FrameClass::EchoOnly; (4.0 / win).ceil() as usize]);
        let samples = vec![0.2f32; 16000 * 4];
        assert!(
            verification_chunk(
                &samples,
                SpeechSupport::default(),
                Some(&evidence),
                0.0,
                4.0
            )
            .is_none(),
            "чужой голос из эха не должен спорить с карточкой"
        );
    }

    #[test]
    fn verification_chunk_masks_silence_by_vad() {
        let samples = vec![0.2f32; 16000 * 4];
        let win = mr_ml::onnx::vad::VAD_WINDOW;
        let mut probs = vec![0.0f32; samples.len() / win];
        let half = probs.len() / 2;
        for prob in probs.iter_mut().skip(half) {
            *prob = 1.0;
        }
        let support = SpeechSupport {
            vad_probs: Some(&probs),
            segmentation: None,
        };
        let chunk = verification_chunk(&samples, support, None, 0.0, 4.0).expect("речь есть");
        assert!(
            (chunk.len() as f64 / 16000.0 - 2.0).abs() < 0.1,
            "{}",
            chunk.len() as f64 / 16000.0
        );
    }

    #[test]
    fn continuation_chain_inherits_previous_matched_card() {
        let prev = neighbor("A", 10.0, true);
        let current = utterance(11.0, 13.0, "Собеседник 1");
        assert!(chain_inherit(&prev, &current, &[0.8, 0.6]));
        assert!(!chain_inherit(
            &neighbor("A", 10.0, false),
            &current,
            &[0.8, 0.6]
        ));
    }

    #[test]
    fn continuation_chain_rejects_different_voice() {
        let prev = neighbor("A", 10.0, true);
        let current = utterance(11.0, 13.0, "Собеседник 1");
        let other = [0.3, 0.91f64.sqrt()];
        assert!(cosine_similarity(&[1.0, 0.0], &other) < CHAIN_MIN_SIM);
        assert!(!chain_inherit(&prev, &current, &other));
    }

    #[test]
    fn continuation_chain_rejects_long_pause() {
        let prev = neighbor("A", 10.0, true);
        let current = utterance(13.0, 15.0, "Собеседник 1");
        assert!(!chain_inherit(&prev, &current, &[1.0, 0.0]));
    }

    #[test]
    fn short_utterance_is_adopted_by_agreeing_neighbors() {
        let mut utterances = vec![
            utterance(0.0, 2.0, "A"),
            utterance(2.5, 3.5, "B"),
            utterance(3.18, 4.09, "Микс"),
            utterance(5.64, 7.5, "A"),
        ];
        utterances[1].channel = Some(1);
        let infos = vec![info("A", true, vec![1.0, 0.0, 0.0])];
        let adopted =
            adopt_short_between_matched_neighbors(&mut utterances, &infos, &HashSet::new());
        assert_eq!(adopted, 1);
        assert_eq!(utterances[2].speaker, "A");
        assert_eq!(utterances[2].silhouette, 0.0);
    }

    #[test]
    fn disagreeing_neighbors_do_not_adopt_short_utterance() {
        let mut different_cards = vec![
            utterance(0.0, 2.0, "A"),
            utterance(3.0, 3.8, "Микс"),
            utterance(5.0, 7.0, "B"),
        ];
        let matched_both = vec![
            info("A", true, vec![1.0, 0.0, 0.0]),
            info("B", true, vec![0.0, 1.0, 0.0]),
        ];
        assert_eq!(
            adopt_short_between_matched_neighbors(
                &mut different_cards,
                &matched_both,
                &HashSet::new()
            ),
            0
        );
        assert_eq!(different_cards[1].speaker, "Микс");

        let mut unmatched = vec![
            utterance(0.0, 2.0, "A"),
            utterance(3.0, 3.8, "Микс"),
            utterance(5.0, 7.0, "A"),
        ];
        let anon = vec![info("A", false, vec![1.0, 0.0, 0.0])];
        assert_eq!(
            adopt_short_between_matched_neighbors(&mut unmatched, &anon, &HashSet::new()),
            0
        );
        assert_eq!(unmatched[1].speaker, "Микс");
    }

    #[test]
    fn abandoned_mix_card_is_adopted_but_stable_matched_card_is_not() {
        let mut utterances = vec![
            utterance(0.0, 2.0, "B"),
            utterance(3.0, 3.8, "Микс"),
            utterance(5.0, 7.0, "B"),
        ];
        let infos = vec![
            info("B", true, vec![1.0, 0.0, 0.0]),
            info("Микс", true, vec![0.0, 1.0, 0.0]),
        ];
        let abandoned = HashSet::from(["Микс".to_string()]);
        assert_eq!(
            adopt_short_between_matched_neighbors(&mut utterances, &infos, &abandoned),
            1
        );
        assert_eq!(utterances[1].speaker, "B");

        let mut stable = vec![
            utterance(0.0, 2.0, "B"),
            utterance(3.0, 3.8, "Микс"),
            utterance(5.0, 7.0, "B"),
        ];
        assert_eq!(
            adopt_short_between_matched_neighbors(&mut stable, &infos, &HashSet::new()),
            0
        );
        assert_eq!(stable[1].speaker, "Микс");
    }

    #[test]
    fn short_utterance_flip_requires_wide_margin() {
        let noisy = vec![profile("A", 0.5), profile("B", 0.5289)];
        assert!(!contradicts(&[1.0, 0.0], "A", &noisy, SHORT_FLIP_MARGIN));
        assert!(contradicts(
            &[1.0, 0.0],
            "A",
            &noisy,
            mr_ml::DEFAULT_MIN_MARGIN
        ));
        let real_flip = vec![profile("A", 0.5), profile("B", 0.8)];
        assert!(contradicts(&[1.0, 0.0], "A", &real_flip, SHORT_FLIP_MARGIN));
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
        let channels = HashMap::from([(0, quiet), (1, samples)]);
        let views: HashMap<usize, &[f32]> = channels
            .iter()
            .map(|(channel, samples)| (*channel, samples.as_slice()))
            .collect();
        let no_vad = HashMap::new();
        run(
            &mut utterances,
            &mut infos,
            &channels,
            VerificationInput {
                samples: &views,
                vad: &no_vad,
                segmentations: &HashMap::new(),
                echo: None,
            },
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
