use std::collections::{HashMap, HashSet};

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;
use mr_ml::{
    cosine_similarity, match_voiceprints_matrix, score_speaker_similarity, weighted_centroid,
    ClusterMatch, SpeakerProfile,
};

use super::clean_speech::SpeechSupport;
use super::diarize::acoustic_units::normalize_unit_rms;
use super::diarize::{local_region, pick_samples, MIN_LOCAL_EMBED_SEC};
use super::echo::EchoEvidence;
use super::experiment::{self, Embedding};
use super::{SpeakerInfo, Utterance};

/// Калибровка `ERes2NetV2`: `onnx_real` проверяет различимость голосов на фрагментах от 1,5 с.
const MIN_VERIFY_SEC: f64 = 1.5;
/// Та же граница «один голос», что в `reconcile_channels`; иначе каждая спорная реплика плодит карточку.
const ANON_MERGE_SIM: f64 = 0.55;
/// Замер живой записи: короткое «угу» 0.91 с зажато соседями того же канала
/// (паузы 1.18/1.55 с на акустической стадии); пословная атрибуция сдвигает
/// границу следующей реплики до 2.08 с, поэтому допуск 2.2 с.
const ADOPT_PAUSE_SEC: f64 = 2.2;
/// Замер `ERes2NetV2` на фронтенд-канале живой записи: соседние куски одного
/// голоса с паузами 0.72–0.78 с — cos 0.6267–0.7606, реплики разных голосов — ≤ 0.378.
const CHAIN_PAUSE_SEC: f64 = 1.5;
const CHAIN_MIN_SIM: f64 = 0.5;
/// Замер живой записи: ложный переворот реплики 1.7 с — маржа 0.0289, настоящие
/// перевороты — маржа 0.30+; на коротких кусках переворачивает только уверенная смена.
const SHORT_FLIP_SEC: f64 = 3.0;
const SHORT_FLIP_MARGIN: f64 = 0.06;
/// Ниже этого сходства со своим профилем карточка точно чужая, даже если
/// альтернативного совпадения в базе голосов нет.
const MIN_CLUSTER_KEEP_SIM: f64 = 0.30;

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

#[derive(Clone, Copy)]
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
    channel: usize,
    start: f64,
    end: f64,
) -> Option<Vec<f32>> {
    const SR: f64 = 16000.0;
    // EchoEvidence описывает эхо в микрофонном канале (ch=0); системный канал —
    // источник речи собеседников, и фильтрация по ch0 вырезала бы их голоса.
    let ch_echo = (channel == 0).then_some(echo).flatten();
    let (start, end) = match ch_echo {
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

fn match_single_embedding(embedding: &[f64], profiles: &[SpeakerProfile]) -> ClusterMatch {
    match_voiceprints_matrix(
        &[embedding.to_vec()],
        profiles,
        mr_ml::DEFAULT_MATCH_THRESHOLD,
        mr_ml::DEFAULT_MIN_MARGIN,
    )
    .remove(0)
}

// Подтверждённая локальная карточка этой записи снимает ложный отсев по глобальному cohort z-score без ослабления порогов.
fn match_verified_embedding(
    embedding: &[f64],
    profiles: &[SpeakerProfile],
    infos: &[SpeakerInfo],
    changed: &HashSet<String>,
    verified: &HashMap<String, Vec<(Vec<f64>, f64)>>,
) -> ClusterMatch {
    let mut result = match_single_embedding(embedding, profiles);
    // Fallback снимает только отсев по глобальному cohort z-score (работает с 4 профилей),
    // чтобы не обходить усиленные пороги для 1–3 профилей.
    if result.matched
        || profiles.len() < 4
        || result.similarity < mr_ml::DEFAULT_MATCH_THRESHOLD
        || result.margin < mr_ml::DEFAULT_MIN_MARGIN
    {
        return result;
    }
    let (mut best, mut second) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut best_profile: Option<&SpeakerProfile> = None;
    for profile in profiles {
        let score = score_speaker_similarity(embedding, profile);
        if score > best {
            second = best;
            best = score;
            best_profile = Some(profile);
        } else if score > second {
            second = score;
        }
    }
    let Some(candidate) = best_profile else {
        return result;
    };
    if best < mr_ml::DEFAULT_MATCH_THRESHOLD
        || (profiles.len() > 1 && best - second < mr_ml::DEFAULT_MIN_MARGIN)
    {
        return result;
    }
    let confirmed = infos
        .iter()
        .filter(|info| {
            info.matched
                && info.name == candidate.name
                && (!changed.contains(&info.name) || verified.contains_key(&info.name))
        })
        .filter_map(|info| info.embedding.as_deref())
        .any(|local| cosine_similarity(embedding, local) >= mr_ml::SAME_SPEAKER_MERGE_SIM);
    if confirmed {
        result.matched = true;
        result.name = Some(candidate.name.clone());
        result.pack_id = (!candidate.pack_id.is_empty()).then(|| candidate.pack_id.clone());
        result.pack_name = (!candidate.pack_name.is_empty()).then(|| candidate.pack_name.clone());
    }
    result
}

fn profile_scores(
    embedding: &[f64],
    assigned: &str,
    profiles: &[SpeakerProfile],
) -> Option<(f64, f64)> {
    let profile = profiles.iter().find(|profile| profile.name == assigned)?;
    let own = score_speaker_similarity(embedding, profile);
    let best = profiles
        .iter()
        .map(|profile| score_speaker_similarity(embedding, profile))
        .fold(own, f64::max);
    Some((own, best))
}

fn contradicts(
    embedding: &[f64],
    assigned: &str,
    profiles: &[SpeakerProfile],
    flip_margin: f64,
) -> bool {
    let Some((own, best)) = profile_scores(embedding, assigned, profiles) else {
        return false;
    };
    own < mr_ml::DEFAULT_MATCH_THRESHOLD || best - own >= flip_margin
}

fn should_split_matched_card(
    embedding: &[f64],
    assigned: &str,
    profiles: &[SpeakerProfile],
    result: &ClusterMatch,
    flip_margin: f64,
) -> bool {
    if result.matched {
        return true;
    }
    let Some((own, best)) = profile_scores(embedding, assigned, profiles) else {
        return false;
    };
    // При шуме кластера ниже порога 0.45, когда ни один профиль не совпал,
    // карточку рвём лишь при явной чужой альтернативе или полном провале сходства.
    own < MIN_CLUSTER_KEEP_SIM
        || (best >= mr_ml::DEFAULT_MATCH_THRESHOLD && best - own >= flip_margin)
}

fn apply_verified_result(
    info: &mut SpeakerInfo,
    named_match: bool,
    result: ClusterMatch,
    embedding: &[f64],
) {
    let matched = named_match && result.matched;
    let (matched_name, matched_pack_id, matched_pack_name, similarity) = if matched {
        (
            result.name,
            result.pack_id,
            result.pack_name,
            result.similarity,
        )
    } else {
        (None, None, None, 0.0)
    };
    info.matched = matched;
    info.matched_name = matched_name;
    info.matched_pack_id = matched_pack_id;
    info.matched_pack_name = matched_pack_name;
    info.similarity = similarity;
    info.silhouette = 0.0;
    info.candidates = result
        .candidates
        .into_iter()
        .map(|candidate| (candidate.name, candidate.similarity))
        .collect();
    info.embedding = Some(embedding.to_vec());
    info.samples.clear();
}

fn ensure_verified_speaker_info(
    infos: &mut Vec<SpeakerInfo>,
    names: &mut HashSet<String>,
    utterance: &Utterance,
    target_name: &str,
    named_match: bool,
    result: ClusterMatch,
    embedding: &[f64],
) {
    if let Some(existing) = infos.iter_mut().find(|info| info.name == target_name) {
        // Несовпавшая карточка с совпадающим именем по умолчанию не подтверждена;
        // при прямом совпадении с профилем повышаем её до matched вместо пропуска.
        if !existing.matched && named_match && result.matched {
            apply_verified_result(existing, true, result, embedding);
        }
        return;
    }
    let mut new_info = infos
        .iter()
        .find(|info| info.name == utterance.speaker)
        .cloned()
        .unwrap_or_else(|| SpeakerInfo {
            id: String::new(),
            name: target_name.to_string(),
            channel: utterance.channel,
            matched: false,
            matched_name: None,
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: 0.0,
            silhouette: 0.0,
            candidates: vec![],
            embedding: None,
            samples: vec![],
            utterances_count: 0,
            total_speech_sec: 0.0,
        });
    new_info.id = format!("verified_{}", infos.len());
    new_info.name = target_name.to_string();
    new_info.channel = utterance.channel;
    apply_verified_result(&mut new_info, named_match, result, embedding);
    infos.push(new_info);
    names.insert(target_name.to_string());
}

#[allow(clippy::too_many_arguments)]
fn reassign_utterance(
    utterance: &mut Utterance,
    infos: &mut Vec<SpeakerInfo>,
    names: &mut HashSet<String>,
    changed: &mut HashSet<String>,
    target_name: String,
    named_match: bool,
    result: ClusterMatch,
    embedding: &[f64],
) -> bool {
    ensure_verified_speaker_info(
        infos,
        names,
        utterance,
        &target_name,
        named_match,
        result,
        embedding,
    );
    if utterance.speaker == target_name {
        return false;
    }
    changed.insert(std::mem::replace(&mut utterance.speaker, target_name));
    utterance.silhouette = 0.0;
    true
}

#[allow(clippy::too_many_arguments)]
fn verify_matched_card(
    utterance: &mut Utterance,
    infos: &mut Vec<SpeakerInfo>,
    names: &mut HashSet<String>,
    changed: &mut HashSet<String>,
    verified: &HashMap<String, Vec<(Vec<f64>, f64)>>,
    next_unknown: &mut usize,
    embedding: &[f64],
    duration: f64,
    profiles: &[SpeakerProfile],
) -> bool {
    let flip_margin = if duration < SHORT_FLIP_SEC {
        SHORT_FLIP_MARGIN
    } else {
        mr_ml::DEFAULT_MIN_MARGIN
    };
    if !contradicts(embedding, &utterance.speaker, profiles, flip_margin) {
        return false;
    }
    let result = match_verified_embedding(embedding, profiles, infos, changed, verified);
    if !should_split_matched_card(
        embedding,
        &utterance.speaker,
        profiles,
        &result,
        flip_margin,
    ) {
        return false;
    }
    let has_matched_card = result.matched
        && result.name.as_ref().is_some_and(|matched_name| {
            infos
                .iter()
                .any(|info| info.name == *matched_name && info.matched)
        });
    // Новое имя профиля требует 2 с чистого аудио; в уже подтверждённую
    // на этом звонке карточку разрешаем переход уверенно с 1.5 с.
    let claim_name = may_claim_name(duration) || has_matched_card;
    let name = if claim_name {
        result.name.clone()
    } else {
        None
    }
    .unwrap_or_else(|| {
        if let Some(known) = nearest_anonymous(infos, embedding, &utterance.speaker) {
            return known.to_string();
        }
        while names.contains(&format!("Собеседник {}", *next_unknown)) {
            *next_unknown += 1;
        }
        let name = format!("Собеседник {}", *next_unknown);
        *next_unknown += 1;
        name
    });
    reassign_utterance(
        utterance, infos, names, changed, name, claim_name, result, embedding,
    )
}

#[allow(clippy::too_many_arguments)]
fn verify_unmatched_utterance(
    utterance: &mut Utterance,
    infos: &mut Vec<SpeakerInfo>,
    names: &mut HashSet<String>,
    changed: &mut HashSet<String>,
    verified: &HashMap<String, Vec<(Vec<f64>, f64)>>,
    chain_prev: Option<&ChannelNeighbor>,
    verify_unmatched_remote: bool,
    embedding: &[f64],
    duration: f64,
    profiles: &[SpeakerProfile],
    reassigned: &mut usize,
    chained: &mut usize,
) -> bool {
    let mut direct_matched = false;
    if verify_unmatched_remote && may_claim_name(duration) {
        let result = match_verified_embedding(embedding, profiles, infos, changed, verified);
        if result.matched {
            if let Some(name) = result.name.clone() {
                direct_matched = true;
                if reassign_utterance(
                    utterance, infos, names, changed, name, true, result, embedding,
                ) {
                    *reassigned += 1;
                }
            }
        }
    }
    if !direct_matched && inherit_allowed(false, chain_prev, utterance, embedding) {
        if let Some(prev) = chain_prev {
            if utterance.speaker != prev.speaker {
                changed.insert(std::mem::replace(
                    &mut utterance.speaker,
                    prev.speaker.clone(),
                ));
                utterance.silhouette = 0.0;
                *chained += 1;
            }
        }
        return true;
    }
    false
}

fn update_verified_cards(
    infos: &mut [SpeakerInfo],
    verified: &mut HashMap<String, Vec<(Vec<f64>, f64)>>,
    changed: &HashSet<String>,
    profiles: &[SpeakerProfile],
) {
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
            info.candidates = match_single_embedding(&embedding, profiles)
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
}

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
    run_with_embedder(utterances, infos, channels, audio, profiles, |chunk| {
        extractor.embed(chunk).ok().flatten()
    });
}

fn embed_utterance(
    audio: VerificationInput<'_>,
    channel: usize,
    utterance: &Utterance,
    embed: &mut impl FnMut(&[f32]) -> Option<Vec<f64>>,
    failures: &mut usize,
) -> Option<(Vec<f64>, f64)> {
    let samples = audio.samples.get(&channel)?;
    let support = SpeechSupport {
        vad_probs: audio.vad.get(&channel).map(Vec::as_slice),
        segmentation: audio.segmentations.get(&channel),
    };
    let chunk = verification_chunk(
        samples,
        support,
        audio.echo,
        channel,
        utterance.start,
        utterance.end,
    )?;
    let duration = chunk.len() as f64 / 16000.0;
    if duration < MIN_VERIFY_SEC {
        return None;
    }
    let Some(embedding) = embed(&chunk) else {
        *failures += 1;
        return None;
    };
    Some((embedding, duration))
}

fn run_with_embedder(
    utterances: &mut Vec<Utterance>,
    infos: &mut Vec<SpeakerInfo>,
    channels: &HashMap<usize, Vec<f32>>,
    audio: VerificationInput<'_>,
    profiles: &[SpeakerProfile],
    mut embed: impl FnMut(&[f32]) -> Option<Vec<f64>>,
) {
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
        let verify_unmatched_remote = !verify_card && channel != 0 && !profiles.is_empty();
        if chain_prev.is_none() && !verify_card && !verify_unmatched_remote {
            continue;
        }
        let Some((embedding, duration)) =
            embed_utterance(audio, channel, utterance, &mut embed, &mut failures)
        else {
            continue;
        };
        let inherited = if verify_card {
            if verify_matched_card(
                utterance,
                infos,
                &mut names,
                &mut changed,
                &verified,
                &mut next_unknown,
                &embedding,
                duration,
                profiles,
            ) {
                reassigned += 1;
            }
            false
        } else {
            verify_unmatched_utterance(
                utterance,
                infos,
                &mut names,
                &mut changed,
                &verified,
                chain_prev,
                verify_unmatched_remote,
                &embedding,
                duration,
                profiles,
                &mut reassigned,
                &mut chained,
            )
        };
        let matched = infos
            .iter()
            .any(|info| info.name == utterance.speaker && info.matched);
        if inherited || matched || verify_unmatched_remote {
            verified
                .entry(utterance.speaker.clone())
                .or_default()
                .push((embedding.clone(), duration));
        }
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
    update_verified_cards(infos, &mut verified, &changed, profiles);
    finalize_verified_utterances(utterances, infos, channels);
    log::debug!(
        target: "diar",
        reassigned = reassigned,
        chained = chained,
        adopted = adopted,
        failures = failures;
        "ev=diar.verify_profiles"
    );
}

fn finalize_verified_utterances(
    utterances: &mut Vec<Utterance>,
    infos: &mut [SpeakerInfo],
    channels: &HashMap<usize, Vec<f32>>,
) {
    super::diarize::dedupe_cross_channel(utterances, infos, channels);
    // Переназначенные и открытые после отсева эха фрагменты одного голоса
    // на одном канале должны собраться обратно в цельные реплики.
    utterances.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.speaker.cmp(&b.speaker)));
    *utterances = super::diarize::merge_utterances(std::mem::take(utterances));
    for info in infos.iter_mut() {
        let members: Vec<&Utterance> = utterances
            .iter()
            .filter(|utterance| utterance.speaker == info.name)
            .collect();
        info.samples = pick_samples(&members, 3);
    }
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
    fn echo_only_region_is_not_verified_on_mic_but_kept_on_system() {
        use crate::transcribe::echo::FrameClass;
        let win = mr_ml::onnx::vad::VAD_WINDOW as f64 / 16000.0;
        let evidence = echo_evidence(vec![FrameClass::EchoOnly; (4.0 / win).ceil() as usize]);
        let samples = vec![0.2f32; 16000 * 4];
        assert!(
            verification_chunk(
                &samples,
                SpeechSupport::default(),
                Some(&evidence),
                0,
                0.0,
                4.0
            )
            .is_none(),
            "чужой голос из эха не должен спорить с карточкой"
        );
        let ch1 = verification_chunk(
            &samples,
            SpeechSupport::default(),
            Some(&evidence),
            1,
            0.0,
            4.0,
        )
        .expect("речь системного канала не режется маской эха микрофона");
        assert_eq!(ch1.len(), 16000 * 4);
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
        let chunk = verification_chunk(&samples, support, None, 0, 0.0, 4.0).expect("речь есть");
        assert!(
            (chunk.len() as f64 / 16000.0 - 2.0).abs() < 0.1,
            "{}",
            chunk.len() as f64 / 16000.0
        );
    }

    #[test]
    fn finalize_merges_reassigned_same_channel_fragments_after_dedupe() {
        let mut u1 = utterance(0.0, 2.0, "Спикер A");
        u1.channel = Some(1);
        u1.text = "альфа бета".to_string();
        let mut echo = utterance(0.2, 2.2, "Спикер B");
        echo.channel = Some(0);
        echo.text = "альфа бета".to_string();
        let mut u2 = utterance(2.5, 4.5, "Спикер A");
        u2.channel = Some(1);
        u2.text = "гамма дельта".to_string();
        let mut delayed_echo = utterance(2.7, 5.1, "Спикер A");
        delayed_echo.channel = Some(0);
        delayed_echo.text = "гамма дельта".to_string();
        let mut next = utterance(4.7, 6.0, "Спикер B");
        next.channel = Some(1);
        next.text = "эпсилон".to_string();

        let mut utterances = vec![u1, echo, u2, delayed_echo, next];
        let mut infos = vec![
            info("Спикер A", true, vec![1.0, 0.0]),
            info("Спикер B", true, vec![0.0, 1.0]),
        ];
        infos[0].channel = Some(1);
        let channels = HashMap::from([
            (0usize, vec![0.05f32; 16000 * 7]),
            (1usize, vec![0.3f32; 16000 * 7]),
        ]);

        finalize_verified_utterances(&mut utterances, &mut infos, &channels);
        assert_eq!(utterances.len(), 2, "{utterances:?}");
        assert_eq!(utterances[0].speaker, "Спикер A");
        assert_eq!(utterances[0].channel, Some(1));
        assert_eq!(utterances[0].text, "альфа бета гамма дельта");
        assert_eq!(utterances[0].start, 0.0);
        assert_eq!(utterances[0].end, 4.5);
        assert!(utterances[0].end <= utterances[1].start);
        assert_eq!(infos[0].samples.len(), 1);
        assert_eq!(infos[0].samples[0].end, 4.5);
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
                .map(|sample| f32::from(sample.expect("PCM")) / 32768.0)
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

    #[test]
    fn noisy_below_threshold_matched_cluster_is_not_split_into_phantom() {
        let mild_noise = vec![profile("A", 0.43), profile("B", 0.41)];
        let unmatched = match_voiceprints_matrix(
            &[vec![1.0, 0.0]],
            &mild_noise,
            mr_ml::DEFAULT_MATCH_THRESHOLD,
            mr_ml::DEFAULT_MIN_MARGIN,
        )
        .remove(0);
        assert!(!unmatched.matched);
        assert!(!should_split_matched_card(
            &[1.0, 0.0],
            "A",
            &mild_noise,
            &unmatched,
            mr_ml::DEFAULT_MIN_MARGIN
        ));

        let unsupported = vec![profile("A", 0.2)];
        let empty = match_voiceprints_matrix(
            &[vec![1.0, 0.0]],
            &unsupported,
            mr_ml::DEFAULT_MATCH_THRESHOLD,
            mr_ml::DEFAULT_MIN_MARGIN,
        )
        .remove(0);
        assert!(should_split_matched_card(
            &[1.0, 0.0],
            "A",
            &unsupported,
            &empty,
            mr_ml::DEFAULT_MIN_MARGIN
        ));

        let ambiguous_alt = vec![
            profile("A", 0.45),
            profile("B", 0.55),
            profile("C", 0.53),
            profile("D", 0.51),
            profile("E", 0.49),
        ];
        let tied = match_voiceprints_matrix(
            &[vec![1.0, 0.0]],
            &ambiguous_alt,
            mr_ml::DEFAULT_MATCH_THRESHOLD,
            mr_ml::DEFAULT_MIN_MARGIN,
        )
        .remove(0);
        assert!(!tied.matched);
        assert!(should_split_matched_card(
            &[1.0, 0.0],
            "A",
            &ambiguous_alt,
            &tied,
            mr_ml::DEFAULT_MIN_MARGIN
        ));
    }

    #[test]
    fn real_voice_matches_unmatched_remote_cluster_utterance() {
        let Some(models) = std::env::var_os("MR_MODELS_DIR") else {
            return;
        };
        let model =
            std::path::PathBuf::from(models).join("diar-eres2netv2-emb/eres2netv2_emb.onnx");
        let mut extractor = EmbeddingExtractor::load(&model).expect("модель ERes2NetV2");
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mr-ml/tests/fixtures");
        let samples: Vec<f32> = hound::WavReader::open(fixtures.join("speaker1_a_cn_16k.wav"))
            .expect("открытый WAV")
            .samples::<i16>()
            .map(|sample| f32::from(sample.expect("PCM")) / 32768.0)
            .collect();
        let own = extractor
            .embed(&samples)
            .expect("инференс")
            .expect("голос A");
        let mut a = profile("A", 0.5);
        a.embedding = own;
        let duration = samples.len() as f64 / 16000.0;
        let mut utt = utterance(0.0, duration, "Собеседник 1");
        utt.channel = Some(1);
        utt.text = "раз два".to_string();
        let mut utterances = vec![utt];
        let mut infos = vec![info("Собеседник 1", false, vec![0.0; mr_ml::EMBEDDING_DIM])];
        infos[0].channel = Some(1);
        let channels = HashMap::from([(1, samples)]);
        let views: HashMap<usize, &[f32]> = channels
            .iter()
            .map(|(channel, s)| (*channel, s.as_slice()))
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
            &[a],
        );
        super::super::diarize::refresh_speaker_stats(&mut infos, &utterances);
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(infos.len(), 1);
        assert!(infos[0].matched);
    }

    #[test]
    fn partial_split_of_unmatched_remote_cluster_keeps_remaining_embedding_and_candidates() {
        let profiles = vec![profile("A", 1.0)];
        let mut u0 = utterance(0.0, 2.5, "Собеседник 1");
        u0.channel = Some(1);
        u0.text = "раз два".to_string();
        let mut u1 = utterance(5.0, 7.5, "Собеседник 1");
        u1.channel = Some(1);
        u1.text = "три четыре".to_string();
        let mut utterances = vec![u0, u1];

        let mut card = info("Собеседник 1", false, vec![0.8, 0.6]);
        card.channel = Some(1);
        let mut infos = vec![card];

        let mut samples = vec![0.2f32; 16000 * 8];
        samples[16000 * 5..].fill(-0.2);
        let channels = HashMap::from([(1usize, samples)]);
        let views: HashMap<usize, &[f32]> = channels
            .iter()
            .map(|(ch, buf)| (*ch, buf.as_slice()))
            .collect();
        let no_vad = HashMap::new();
        let remaining_vec = vec![0.42, (1.0f64 - 0.42 * 0.42).sqrt()];
        let embed_vec = remaining_vec.clone();
        run_with_embedder(
            &mut utterances,
            &mut infos,
            &channels,
            VerificationInput {
                samples: &views,
                vad: &no_vad,
                segmentations: &HashMap::new(),
                echo: None,
            },
            &profiles,
            move |chunk| {
                if chunk[0] > 0.0 {
                    Some(vec![1.0, 0.0])
                } else {
                    Some(embed_vec.clone())
                }
            },
        );
        super::super::diarize::refresh_speaker_stats(&mut infos, &utterances);

        assert_eq!(utterances.len(), 2, "{utterances:?}");
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(utterances[1].speaker, "Собеседник 1");
        let remaining = infos
            .iter()
            .find(|i| i.name == "Собеседник 1")
            .expect("оставшаяся нематченная карточка сохранена");
        assert!(!remaining.matched);
        let rem_emb = remaining
            .embedding
            .as_ref()
            .expect("эмбеддинг оставшегося кластера сохранён");
        assert!(cosine_similarity(rem_emb, &remaining_vec) > 0.9999);
        assert_eq!(remaining.candidates.len(), 1);
        assert_eq!(remaining.candidates[0].0, "A");
    }

    #[test]
    fn colliding_unmatched_label_cannot_claim_short_flip_and_upgrades_on_full_match() {
        let profiles = vec![profile("A", 1.0), profile("B", 0.0)];
        let mut u0 = utterance(0.0, 1.7, "A");
        u0.channel = Some(1);
        u0.text = "короткая".to_string();
        let mut u1 = utterance(5.0, 7.5, "A");
        u1.channel = Some(1);
        u1.text = "длинная".to_string();
        let mut utterances = vec![u0, u1];

        let mut card_a = info("A", true, vec![1.0, 0.0]);
        card_a.channel = Some(1);
        let mut card_b_unmatched = info("B", false, vec![1.0, 0.0]);
        card_b_unmatched.channel = Some(1);
        let mut infos = vec![card_a, card_b_unmatched];

        let channels = HashMap::from([(1usize, vec![0.2f32; 16000 * 8])]);
        let views: HashMap<usize, &[f32]> = channels
            .iter()
            .map(|(ch, buf)| (*ch, buf.as_slice()))
            .collect();
        let no_vad = HashMap::new();
        run_with_embedder(
            &mut utterances,
            &mut infos,
            &channels,
            VerificationInput {
                samples: &views,
                vad: &no_vad,
                segmentations: &HashMap::new(),
                echo: None,
            },
            &profiles,
            |_| Some(vec![0.0, 1.0]),
        );

        assert_eq!(
            utterances[0].speaker, "Собеседник 1",
            "короткий кусок < 2 с не должен переходить в нематченную карточку B"
        );
        assert_eq!(utterances[1].speaker, "B");
        let b_card = infos.iter().find(|i| i.name == "B").expect("карточка B");
        assert!(
            b_card.matched,
            "прямое совпадение ≥ 2 с переводит карточку в matched"
        );
    }

    #[test]
    fn unmatched_remote_long_utterance_prefers_direct_profile_over_chain_inherit() {
        let profiles = vec![profile("A", 1.0), profile("B", 0.55)];
        let mut u0 = utterance(0.0, 2.5, "A");
        u0.channel = Some(1);
        u0.text = "первая".to_string();
        let mut u1 = utterance(3.0, 5.5, "Собеседник 1");
        u1.channel = Some(1);
        u1.text = "вторая".to_string();
        let mut u2 = utterance(6.7, 8.4, "Собеседник 2");
        u2.channel = Some(1);
        u2.text = "третья".to_string();
        let mut utterances = vec![u0, u1, u2];

        let mut card_a = info("A", true, vec![1.0, 0.0]);
        card_a.channel = Some(1);
        let mut card_u1 = info(
            "Собеседник 1",
            false,
            vec![0.55, (1.0f64 - 0.55 * 0.55).sqrt()],
        );
        card_u1.channel = Some(1);
        let mut card_u2 = info(
            "Собеседник 2",
            false,
            vec![0.55, (1.0f64 - 0.55 * 0.55).sqrt()],
        );
        card_u2.channel = Some(1);
        let mut infos = vec![card_a, card_u1, card_u2];

        let mut samples = vec![0.2f32; 16000 * 9];
        samples[16000 * 3..].fill(-0.2);
        let channels = HashMap::from([(1usize, samples)]);
        let views: HashMap<usize, &[f32]> = channels
            .iter()
            .map(|(ch, buf)| (*ch, buf.as_slice()))
            .collect();
        let no_vad = HashMap::new();
        let b_vec = profiles[1].embedding.clone();
        run_with_embedder(
            &mut utterances,
            &mut infos,
            &channels,
            VerificationInput {
                samples: &views,
                vad: &no_vad,
                segmentations: &HashMap::new(),
                echo: None,
            },
            &profiles,
            |chunk| {
                if chunk[0] > 0.0 {
                    Some(vec![1.0, 0.0])
                } else {
                    Some(b_vec.clone())
                }
            },
        );

        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(
            utterances[1].speaker, "B",
            "длинная реплика ≥ 2 с с прямым профилем B не должна наследоваться в A"
        );
        assert_eq!(
            utterances[2].speaker, "B",
            "короткая < 2 с продолжает наследовать ближайшего соседа"
        );
    }

    fn axis_vec(first: f64, dim: usize) -> Vec<f64> {
        let mut v = vec![0.0; 8];
        v[0] = first;
        if dim != 0 {
            v[dim] = (1.0 - first * first).sqrt();
        }
        v
    }

    fn axis_profile(name: &str, first: f64, dim: usize) -> SpeakerProfile {
        SpeakerProfile {
            name: name.to_string(),
            pack_id: String::new(),
            pack_name: String::new(),
            embedding: axis_vec(first, dim),
            samples_count: 1,
            updated_at: String::new(),
            prototypes: vec![],
        }
    }

    fn cohort_rejected_profiles() -> Vec<SpeakerProfile> {
        let mut a = axis_profile("A", 0.58, 1);
        a.pack_id = "pack-1".to_string();
        a.pack_name = "Пакет 1".to_string();
        vec![
            a,
            axis_profile("B", 0.56, 2),
            axis_profile("C", 0.53, 3),
            axis_profile("D", 0.52, 4),
            axis_profile("E", 0.51, 5),
        ]
    }

    fn match_verified(
        embedding: &[f64],
        profiles: &[SpeakerProfile],
        infos: &[SpeakerInfo],
    ) -> ClusterMatch {
        match_verified_embedding(embedding, profiles, infos, &HashSet::new(), &HashMap::new())
    }

    #[test]
    fn cohort_rejected_candidate_is_confirmed_by_local_matched_card() {
        let profiles = cohort_rejected_profiles();
        let query = axis_vec(1.0, 0);
        let global = match_single_embedding(&query, &profiles);
        assert!(
            !global.matched,
            "глобальный матчер обязан отсеять по cohort z"
        );

        let local_a = axis_vec(0.71, 1);
        let infos = vec![info("A", true, local_a)];
        let fallback = match_verified(&query, &profiles, &infos);
        assert!(fallback.matched);
        assert_eq!(fallback.name.as_deref(), Some("A"));
        assert_eq!(fallback.pack_id.as_deref(), Some("pack-1"));
        assert_eq!(fallback.pack_name.as_deref(), Some("Пакет 1"));
    }

    #[test]
    fn cohort_rejected_fallback_requires_confirmed_local_card_and_cosine() {
        let profiles = cohort_rejected_profiles();
        let query = axis_vec(1.0, 0);

        let low_sim = vec![info("A", true, axis_vec(0.50, 1))];
        assert!(!match_verified(&query, &profiles, &low_sim).matched);

        let other_profile = vec![info("B", true, query.clone())];
        assert!(!match_verified(&query, &profiles, &other_profile).matched);

        let unmatched = vec![info("A", false, query.clone())];
        assert!(!match_verified(&query, &profiles, &unmatched).matched);

        let mut no_emb = info("A", true, query.clone());
        no_emb.embedding = None;
        assert!(!match_verified(&query, &profiles, &[no_emb]).matched);

        assert!(!match_verified(&query, &profiles, &[]).matched);
    }

    #[test]
    fn local_card_fallback_never_bypasses_similarity_or_margin() {
        let query = axis_vec(1.0, 0);
        let infos = vec![info("A", true, query.clone())];

        let below_thresh = vec![axis_profile("A", 0.44, 1)];
        assert!(!match_verified(&query, &below_thresh, &infos).matched);

        let rounded_thresh = vec![axis_profile("A", 0.4496, 1)];
        assert!(!match_verified(&query, &rounded_thresh, &infos).matched);

        let below_margin = vec![axis_profile("A", 0.55, 1), axis_profile("B", 0.54, 2)];
        assert!(!match_verified(&query, &below_margin, &infos).matched);

        let rounded_margin = vec![
            axis_profile("A", 0.55, 1),
            axis_profile("B", 0.5354, 2),
            axis_profile("C", 0.53, 3),
            axis_profile("D", 0.52, 4),
            axis_profile("E", 0.51, 5),
        ];
        assert!(!match_verified(&query, &rounded_margin, &infos).matched);
    }

    #[test]
    fn abandoned_matched_card_cannot_confirm_cohort_rejected_fallback() {
        let profiles = cohort_rejected_profiles();
        let query = axis_vec(1.0, 0);
        let a_local = axis_vec(0.71, 1);

        let changed = HashSet::from(["A".to_string()]);
        let empty_verified = HashMap::new();
        let infos = vec![info("A", true, a_local.clone())];
        assert!(
            !match_verified_embedding(&query, &profiles, &infos, &changed, &empty_verified).matched
        );

        let active_verified = HashMap::from([("A".to_string(), vec![(a_local.clone(), 3.0)])]);
        assert!(
            match_verified_embedding(&query, &profiles, &infos, &changed, &active_verified).matched
        );

        let mut u_abandoned = utterance(0.0, 3.0, "A");
        u_abandoned.channel = Some(1);
        let mut u_unmatched = utterance(5.0, 9.0, "Собеседник 1");
        u_unmatched.channel = Some(1);
        let mut utterances = vec![u_abandoned, u_unmatched];

        let mut card_a = info("A", true, a_local);
        card_a.channel = Some(1);
        let mut card_b = info("B", true, axis_vec(0.0, 2));
        card_b.channel = Some(1);
        let mut card_u = info("Собеседник 1", false, axis_vec(0.5, 6));
        card_u.channel = Some(1);
        let mut run_infos = vec![card_a, card_b, card_u];

        let mut samples = vec![0.2f32; 16000 * 10];
        samples[16000 * 4..].fill(-0.2);
        let channels = HashMap::from([(1usize, samples)]);
        let views: HashMap<usize, &[f32]> = channels
            .iter()
            .map(|(ch, buf)| (*ch, buf.as_slice()))
            .collect();
        let no_vad = HashMap::new();
        run_with_embedder(
            &mut utterances,
            &mut run_infos,
            &channels,
            VerificationInput {
                samples: &views,
                vad: &no_vad,
                segmentations: &HashMap::new(),
                echo: None,
            },
            &profiles,
            move |chunk| {
                if chunk[0] > 0.0 {
                    Some(axis_vec(0.0, 2))
                } else {
                    Some(query.clone())
                }
            },
        );

        assert_eq!(utterances[0].speaker, "B");
        assert_eq!(utterances[1].speaker, "Собеседник 1");
        assert!(!run_infos.iter().any(|i| i.name == "A" && i.matched));
    }

    #[test]
    fn both_verification_branches_use_local_card_fallback() {
        let profiles = cohort_rejected_profiles();
        let query = axis_vec(1.0, 0);
        let a_local = axis_vec(0.71, 1);

        let mut u_keep = utterance(0.0, 3.0, "A");
        u_keep.channel = Some(1);
        let mut u_split = utterance(5.0, 9.0, "B");
        u_split.channel = Some(1);
        let mut u_unmatched = utterance(11.0, 15.0, "Собеседник 1");
        u_unmatched.channel = Some(1);
        let mut utterances = vec![u_keep, u_split, u_unmatched];

        let mut card_a = info("A", true, a_local.clone());
        card_a.channel = Some(1);
        let mut card_b = info("B", true, axis_vec(0.0, 2));
        card_b.channel = Some(1);
        let mut card_u = info("Собеседник 1", false, axis_vec(0.5, 6));
        card_u.channel = Some(1);
        let mut infos = vec![card_a, card_b, card_u];

        let mut samples = vec![0.2f32; 16000 * 16];
        samples[16000 * 4..].fill(-0.2);
        let channels = HashMap::from([(1usize, samples)]);
        let views: HashMap<usize, &[f32]> = channels
            .iter()
            .map(|(ch, buf)| (*ch, buf.as_slice()))
            .collect();
        let no_vad = HashMap::new();
        run_with_embedder(
            &mut utterances,
            &mut infos,
            &channels,
            VerificationInput {
                samples: &views,
                vad: &no_vad,
                segmentations: &HashMap::new(),
                echo: None,
            },
            &profiles,
            move |chunk| {
                if chunk[0] > 0.0 {
                    Some(a_local.clone())
                } else {
                    Some(query.clone())
                }
            },
        );

        assert_eq!(utterances.len(), 3);
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(
            utterances[1].speaker, "A",
            "ветка verify_matched_card должна подтверждать кандидата через локальную карточку"
        );
        assert_eq!(
            utterances[2].speaker, "A",
            "ветка verify_unmatched_utterance должна подтверждать кандидата через локальную карточку"
        );
        assert!(infos
            .iter()
            .find(|i| i.name == "A")
            .is_some_and(|i| i.matched));
        assert!(infos
            .iter()
            .find(|i| i.name == "B")
            .is_some_and(|i| !i.matched && i.embedding.is_none()));
        assert!(infos
            .iter()
            .find(|i| i.name == "Собеседник 1")
            .is_some_and(|i| !i.matched && i.embedding.is_none()));
    }
}
