use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use mr_ml::cluster::CENTROID_MERGE_SIM;
use mr_ml::{cosine_similarity, weighted_centroid, HIGH_CONFIDENCE_THRESHOLD};

use crate::transcribe::{SpeakerInfo, Utterance};

use super::super::pick_samples;

const MIN_CARD_SPEECH_SEC: f64 = 1.5;

struct CardStats {
    speech: f64,
    dominant: Option<i32>,
}

struct Target {
    index: usize,
    name: String,
    matched: bool,
    similarity: f64,
}

/// После сверки реплик один голос бывает раздроблен на карточки с разными
/// слабыми профилями; хвост ≤10% речи группы крепим по косинусу с её
/// крупнейшей карточкой только для слабого кандидата, а более крупные и
/// уверенные — по полной связи. Анонимную группу без профиля не склеиваем,
/// иначе два равных незнакомых голоса сливаются.
pub(crate) fn merge_verified_cards(
    infos: &mut Vec<SpeakerInfo>,
    utterances: &mut [Utterance],
    is_stereo: bool,
) -> bool {
    if infos.len() < 2 {
        return false;
    }
    let before = infos.len();
    let stats = card_stats(infos, utterances);
    let mut participating: HashSet<&str> = HashSet::new();
    let mut eligible_count = 0usize;
    for (index, info) in infos.iter().enumerate() {
        if info.embedding.is_some() && stats[index].speech >= MIN_CARD_SPEECH_SEC {
            eligible_count += 1;
            participating.insert(info.name.as_str());
        }
    }
    if participating.len() != eligible_count {
        log::debug!(
            target: "diar",
            reason = "skipped_duplicate_names",
            cards = infos.len(),
            duplicates = eligible_count - participating.len();
            "ev=diar.merge_cards"
        );
        return false;
    }
    let skipped_no_embedding = infos.iter().filter(|info| info.embedding.is_none()).count();
    let skipped_short = (0..infos.len())
        .filter(|&index| {
            infos[index].embedding.is_some() && stats[index].speech < MIN_CARD_SPEECH_SEC
        })
        .count();
    let mut order: Vec<usize> = (0..infos.len())
        .filter(|&index| {
            infos[index].embedding.is_some() && stats[index].speech >= MIN_CARD_SPEECH_SEC
        })
        .collect();
    order.sort_by(|&a, &b| {
        stats[b]
            .speech
            .total_cmp(&stats[a].speech)
            .then_with(|| infos[a].name.cmp(&infos[b].name))
    });
    let mut absorbed = Vec::new();
    let mut merged_groups = 0usize;
    let mut skipped_anonymous_groups = 0usize;
    let mut moved = 0usize;
    for group in group_cards(infos, &stats, &order) {
        let has_matched = group.iter().any(|&index| infos[index].matched);
        if !has_matched {
            skipped_anonymous_groups += 1;
            continue;
        }
        let Some(target) = resolve_target(infos, &stats, &group, is_stereo) else {
            log::debug!(
                target: "diar",
                reason = "many_high_confidence",
                cards = group.len();
                "ev=diar.merge_cards"
            );
            continue;
        };
        if target.matched {
            let collision = infos.iter().enumerate().any(|(index, info)| {
                !group.contains(&index)
                    && (info.name == target.name
                        || info.matched_name.as_deref() == Some(target.name.as_str()))
            });
            if collision {
                log::debug!(
                    target: "diar",
                    reason = "named_target_collision",
                    cards = group.len();
                    "ev=diar.merge_cards"
                );
                continue;
            }
        }
        moved += apply_group(infos, utterances, &stats, &group, &target);
        merged_groups += 1;
        absorbed.extend(group.iter().copied().filter(|&index| index != target.index));
    }
    absorbed.sort_unstable();
    absorbed.dedup();
    for index in absorbed.into_iter().rev() {
        infos.remove(index);
    }
    log::debug!(
        target: "diar",
        cards_before = before,
        cards_after = infos.len(),
        groups_merged = merged_groups,
        skipped_anonymous_groups = skipped_anonymous_groups,
        skipped_no_embedding = skipped_no_embedding,
        skipped_short = skipped_short,
        utterances_moved = moved;
        "ev=diar.merge_cards"
    );
    merged_groups > 0
}

fn card_stats(infos: &[SpeakerInfo], utterances: &[Utterance]) -> Vec<CardStats> {
    let index: HashMap<&str, Vec<usize>> = {
        let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
        for (card, info) in infos.iter().enumerate() {
            index.entry(info.name.as_str()).or_default().push(card);
        }
        index
    };
    let mut channels: Vec<HashMap<Option<i32>, f64>> = vec![HashMap::new(); infos.len()];
    for utterance in utterances {
        let Some(cards) = index.get(utterance.speaker.as_str()) else {
            continue;
        };
        for &card in cards {
            *channels[card].entry(utterance.channel).or_insert(0.0) +=
                (utterance.end - utterance.start).max(0.0);
        }
    }
    channels
        .into_iter()
        .map(|channels| {
            let mut speech = 0.0;
            let mut dominant = None;
            for (&channel, &seconds) in &channels {
                speech += seconds;
                let better = match dominant {
                    None => true,
                    Some((best_seconds, best_channel)) => match seconds.total_cmp(&best_seconds) {
                        Ordering::Greater => true,
                        Ordering::Equal => channel_key(channel) < channel_key(best_channel),
                        Ordering::Less => false,
                    },
                };
                if better {
                    dominant = Some((seconds, channel));
                }
            }
            CardStats {
                speech,
                dominant: dominant.and_then(|(_, channel)| channel),
            }
        })
        .collect()
}

fn channel_key(channel: Option<i32>) -> (bool, i32) {
    (channel.is_none(), channel.unwrap_or(i32::MAX))
}

fn group_cards(infos: &[SpeakerInfo], stats: &[CardStats], order: &[usize]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &index in order {
        let Some(embedding) = infos[index].embedding.as_deref() else {
            continue;
        };
        let channel = stats[index].dominant;
        if let Some(group) = groups.iter_mut().find(|group| {
            if stats[group[0]].dominant != channel {
                return false;
            }
            let anchor_matches = infos[group[0]]
                .embedding
                .as_deref()
                .is_some_and(|anchor| cosine_similarity(embedding, anchor) >= CENTROID_MERGE_SIM);
            if !anchor_matches {
                return false;
            }
            let group_speech: f64 = group.iter().map(|&member| stats[member].speech).sum();
            let weak_tail =
                !infos[index].matched || infos[index].similarity < HIGH_CONFIDENCE_THRESHOLD;
            (weak_tail && stats[index].speech <= 0.10 * group_speech)
                || group.iter().all(|&member| {
                    infos[member].embedding.as_deref().is_some_and(|other| {
                        cosine_similarity(embedding, other) >= CENTROID_MERGE_SIM
                    })
                })
        }) {
            group.push(index);
        } else {
            groups.push(vec![index]);
        }
    }
    groups.retain(|group| group.len() >= 2);
    groups
}

fn resolve_target(
    infos: &[SpeakerInfo],
    stats: &[CardStats],
    group: &[usize],
    is_stereo: bool,
) -> Option<Target> {
    let mut confidence: HashMap<&str, f64> = HashMap::new();
    for &index in group {
        let info = &infos[index];
        if !info.matched {
            continue;
        }
        let name = info.matched_name.as_deref().unwrap_or(info.name.as_str());
        confidence
            .entry(name)
            .and_modify(|score| *score = score.max(info.similarity))
            .or_insert(info.similarity);
    }
    let anonymous_speech: f64 = group
        .iter()
        .filter(|&&index| !infos[index].matched)
        .map(|&index| stats[index].speech)
        .sum();
    let total_speech: f64 = group.iter().map(|&index| stats[index].speech).sum();
    let substantial_anonymous = anonymous_speech >= 2.0 && anonymous_speech >= 0.25 * total_speech;
    if confidence.is_empty() {
        return Some(anonymous_target(infos, stats, group, is_stereo));
    }
    if confidence.len() == 1 {
        if let Some((&name, &similarity)) = confidence.iter().next() {
            if similarity >= HIGH_CONFIDENCE_THRESHOLD || !substantial_anonymous {
                return Some(named_target(infos, stats, group, name, similarity));
            }
        }
        return Some(anonymous_target(infos, stats, group, is_stereo));
    }
    let confident = confidence
        .values()
        .filter(|&&score| score >= HIGH_CONFIDENCE_THRESHOLD)
        .count();
    if confident > 1 {
        return None;
    }
    if confident == 0 {
        return Some(anonymous_target(infos, stats, group, is_stereo));
    }
    let (&name, &similarity) = confidence
        .iter()
        .find(|(_, &score)| score >= HIGH_CONFIDENCE_THRESHOLD)?;
    Some(named_target(infos, stats, group, name, similarity))
}

fn named_target(
    infos: &[SpeakerInfo],
    stats: &[CardStats],
    group: &[usize],
    name: &str,
    similarity: f64,
) -> Target {
    let index = group
        .iter()
        .copied()
        .filter(|&index| {
            infos[index]
                .matched_name
                .as_deref()
                .unwrap_or(infos[index].name.as_str())
                == name
        })
        .max_by(|&a, &b| stats[a].speech.total_cmp(&stats[b].speech))
        .unwrap_or(group[0]);
    Target {
        index,
        name: name.to_string(),
        matched: true,
        similarity,
    }
}

fn anonymous_target(
    infos: &[SpeakerInfo],
    stats: &[CardStats],
    group: &[usize],
    is_stereo: bool,
) -> Target {
    if let Some(index) = group.iter().copied().find(|&index| !infos[index].matched) {
        return Target {
            index,
            name: infos[index].name.clone(),
            matched: false,
            similarity: 0.0,
        };
    }
    let prefix = if is_stereo && stats[group[0]].dominant == Some(0) {
        "Участник"
    } else {
        "Собеседник"
    };
    let mut used: HashSet<&str> = infos.iter().map(|info| info.name.as_str()).collect();
    used.extend(
        infos
            .iter()
            .flat_map(|info| info.candidates.iter().map(|(name, _)| name.as_str())),
    );
    used.extend(infos.iter().filter_map(|info| info.matched_name.as_deref()));
    let index = group
        .iter()
        .copied()
        .max_by(|&a, &b| stats[a].speech.total_cmp(&stats[b].speech))
        .unwrap_or(group[0]);
    let mut n = 1usize;
    loop {
        let name = format!("{prefix} {n}");
        if !used.contains(name.as_str()) {
            return Target {
                index,
                name,
                matched: false,
                similarity: 0.0,
            };
        }
        n += 1;
    }
}

fn apply_group(
    infos: &mut [SpeakerInfo],
    utterances: &mut [Utterance],
    stats: &[CardStats],
    group: &[usize],
    target: &Target,
) -> usize {
    let target_old = infos[target.index].name.clone();
    let mut absorbed_names: HashSet<String> = group
        .iter()
        .copied()
        .filter(|&index| index != target.index)
        .map(|index| infos[index].name.clone())
        .collect();
    if target_old != target.name {
        absorbed_names.insert(target_old);
    }
    let mut moved = 0usize;
    for utterance in utterances.iter_mut() {
        if utterance.speaker != target.name && absorbed_names.contains(utterance.speaker.as_str()) {
            utterance.speaker.clone_from(&target.name);
            utterance.silhouette = 0.0;
            moved += 1;
        }
    }
    let target_utterances: Vec<&Utterance> = utterances
        .iter()
        .filter(|utterance| utterance.speaker == target.name)
        .collect();
    let samples = pick_samples(&target_utterances, 3);
    let mut vectors = Vec::new();
    let mut weights = Vec::new();
    let mut candidates: HashMap<String, (String, f64)> = HashMap::new();
    let mut silhouette = 0.0;
    let mut speech = 0.0;
    for &index in group {
        let info = &infos[index];
        if let Some(embedding) = &info.embedding {
            vectors.push(embedding.clone());
            weights.push(stats[index].speech.max(0.1));
        }
        silhouette += info.silhouette * stats[index].speech;
        speech += stats[index].speech;
        for (name, score) in &info.candidates {
            candidates
                .entry(name.to_lowercase())
                .and_modify(|entry| entry.1 = entry.1.max(*score))
                .or_insert_with(|| (name.clone(), *score));
        }
    }
    let mut merged = infos[target.index].clone();
    merged.name.clone_from(&target.name);
    merged.matched = target.matched;
    merged.similarity = target.similarity;
    if target.matched {
        merged.matched_name = Some(target.name.clone());
    } else {
        merged.matched_name = None;
        merged.matched_pack_id = None;
        merged.matched_pack_name = None;
    }
    merged.channel = stats[target.index].dominant.or(merged.channel);
    merged.embedding = if vectors.is_empty() {
        None
    } else {
        Some(weighted_centroid(&vectors, &weights))
    };
    let mut candidates: Vec<(String, f64)> = candidates.into_values().collect();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    merged.candidates = candidates;
    merged.silhouette = if speech > 0.0 {
        silhouette / speech
    } else {
        0.0
    };
    merged.samples = samples;
    infos[target.index] = merged;
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(
        id: &str,
        name: &str,
        embedding: Vec<f64>,
        matched: bool,
        similarity: f64,
        silhouette: f64,
    ) -> SpeakerInfo {
        SpeakerInfo {
            id: id.to_string(),
            name: name.to_string(),
            channel: Some(1),
            matched,
            matched_name: matched.then(|| name.to_string()),
            matched_pack_id: None,
            matched_pack_name: None,
            similarity,
            silhouette,
            candidates: if matched {
                vec![(name.to_string(), similarity)]
            } else {
                Vec::new()
            },
            embedding: Some(embedding),
            samples: Vec::new(),
            utterances_count: 0,
            total_speech_sec: 0.0,
        }
    }

    fn utterance(start: f64, end: f64, speaker: &str, channel: i32) -> Utterance {
        Utterance {
            start,
            end,
            speaker: speaker.to_string(),
            channel: Some(channel),
            text: "тестовая реплика".to_string(),
            diarized: true,
            silhouette: 0.5,
            words: Vec::new(),
            ctc_used: false,
        }
    }

    fn sim_vector(index: usize, count: usize, similarity: f64) -> Vec<f64> {
        let mut vector = vec![0.0; count + 1];
        vector[index] = (1.0 - similarity).sqrt();
        vector[count] = similarity.sqrt();
        vector
    }

    fn utts_for(infos: &[SpeakerInfo], channel: i32) -> Vec<Utterance> {
        infos
            .iter()
            .enumerate()
            .map(|(index, info)| {
                let start = index as f64 * 10.0;
                utterance(start, start + 10.0, &info.name, channel)
            })
            .collect()
    }

    #[test]
    fn weak_conflicting_profile_names_collapse_to_one_anonymous_card() {
        let mut infos = vec![
            card("a", "A", sim_vector(0, 5, 0.8), true, 0.495, 0.5),
            card("b", "B", sim_vector(1, 5, 0.8), true, 0.512, 0.5),
            card("c", "C", sim_vector(2, 5, 0.8), true, 0.552, 0.5),
            card("d", "D", sim_vector(3, 5, 0.8), true, 0.558, 0.5),
            card("e", "Собеседник 1", sim_vector(4, 5, 0.8), false, 0.0, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert!(info.matched_name.is_none());
        assert_eq!(info.similarity, 0.0);
        assert_eq!(info.name, "Собеседник 1");
        assert!(utterances.iter().all(|u| u.speaker == "Собеседник 1"));
        assert_eq!(info.samples.len(), 3);
        assert_eq!(info.candidates.len(), 4);
    }

    #[test]
    fn single_matched_name_absorbs_anonymous_fragments() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.80, 0.6),
            card("b", "Собеседник 1", vec![0.8, 0.6], false, 0.0, 0.2),
        ];
        let mut utterances = vec![
            utterance(0.0, 10.0, "A", 1),
            utterance(10.0, 40.0, "Собеседник 1", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert_eq!(info.name, "A");
        assert!(info.matched);
        assert_eq!(info.similarity, 0.80);
        assert!((info.silhouette - 0.3).abs() < 1e-9);
        assert!(utterances.iter().all(|u| u.speaker == "A"));
    }

    #[test]
    fn one_confident_name_absorbs_weak_conflicts() {
        let mut infos = vec![
            card("a", "A", sim_vector(0, 3, 0.8), true, 0.82, 0.5),
            card("b", "B", sim_vector(1, 3, 0.8), true, 0.51, 0.5),
            card("c", "C", sim_vector(2, 3, 0.8), true, 0.55, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "A");
        assert!(infos[0].matched);
        assert!(utterances.iter().all(|u| u.speaker == "A"));
    }

    #[test]
    fn two_confident_names_block_the_merge() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.82, 0.5),
            card("b", "B", vec![0.8, 0.6], true, 0.80, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "A");
        assert_eq!(infos[1].name, "B");
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(utterances[1].speaker, "B");
    }

    #[test]
    fn different_dominant_channels_are_never_merged() {
        let mut first = card("a", "Участник 1", vec![1.0, 0.0], false, 0.0, 0.5);
        first.channel = Some(0);
        let second = card(
            "b",
            "Собеседник 1",
            vec![0.9, (1.0f64 - 0.81).sqrt()],
            false,
            0.0,
            0.5,
        );
        let mut infos = vec![first, second];
        let mut utterances = vec![
            utterance(0.0, 10.0, "Участник 1", 0),
            utterance(10.0, 20.0, "Собеседник 1", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, true);

        assert_eq!(infos.len(), 2);
    }

    #[test]
    fn short_card_is_not_merged() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.80, 0.5),
            card(
                "b",
                "Собеседник 1",
                vec![0.9, (1.0f64 - 0.81).sqrt()],
                false,
                0.0,
                0.5,
            ),
        ];
        let mut utterances = vec![
            utterance(0.0, 10.0, "A", 1),
            utterance(10.0, 11.0, "Собеседник 1", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 2);
    }

    #[test]
    fn bridge_card_does_not_join_two_voices() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], true, 0.80, 0.5),
            card("b", "Собеседник 2", vec![0.766, 0.643], false, 0.0, 0.5),
            card("c", "Собеседник 3", vec![0.4, 0.9165], false, 0.0, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 2);
        assert_eq!(utterances[0].speaker, utterances[1].speaker);
        assert_ne!(utterances[2].speaker, utterances[0].speaker);
    }

    #[test]
    fn largest_bridge_card_does_not_join_two_voices() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], true, 0.80, 0.5),
            card("b", "Собеседник 2", vec![0.766, 0.643], false, 0.0, 0.5),
            card("c", "Собеседник 3", vec![0.4, 0.9165], false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 7.0, "Собеседник 1", 1),
            utterance(7.0, 17.0, "Собеседник 2", 1),
            utterance(17.0, 24.0, "Собеседник 3", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 2);
        assert_eq!(utterances[0].speaker, utterances[1].speaker);
        assert_ne!(utterances[2].speaker, utterances[0].speaker);
    }

    #[test]
    fn true_bridge_largest_does_not_merge_outers() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("b", "Собеседник 2", vec![0.766, 0.643], true, 0.80, 0.5),
            card("c", "Собеседник 3", vec![0.4, 0.9165], false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 7.0, "Собеседник 1", 1),
            utterance(7.0, 37.0, "Собеседник 2", 1),
            utterance(37.0, 44.0, "Собеседник 3", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 2);
        assert_eq!(utterances[0].speaker, utterances[1].speaker);
        assert_ne!(utterances[2].speaker, utterances[0].speaker);
    }

    #[test]
    fn small_fragment_attaches_through_largest_representative() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("b", "Собеседник 2", vec![0.8, -0.6], false, 0.0, 0.5),
            card("c", "Собеседник 3", vec![0.7, 0.714], true, 0.48, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "Собеседник 1", 1),
            utterance(100.0, 110.0, "Собеседник 2", 1),
            utterance(110.0, 116.9, "Собеседник 3", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "Собеседник 1");
        assert_eq!(utterances.len(), 3);
        assert!(utterances.iter().all(|u| u.speaker == "Собеседник 1"));
    }

    #[test]
    fn high_confidence_tail_does_not_join_anonymous_group() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("b", "A", vec![0.8, -0.6], true, 0.48, 0.5),
            card("c", "B", vec![0.7, 0.714], true, 0.80, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "Собеседник 1", 1),
            utterance(100.0, 110.0, "A", 1),
            utterance(110.0, 117.0, "B", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 2);
        let merged = infos
            .iter()
            .find(|info| info.name == "Собеседник 1")
            .expect("анонимная пара должна остаться");
        assert!(!merged.matched);
        assert_eq!(utterances[0].speaker, "Собеседник 1");
        assert_eq!(utterances[1].speaker, "Собеседник 1");
        assert_eq!(utterances[2].speaker, "B");
    }

    #[test]
    fn anonymous_comparable_cards_are_not_merged() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card(
                "b",
                "Собеседник 2",
                vec![0.9, (1.0f64 - 0.81).sqrt()],
                false,
                0.0,
                0.5,
            ),
        ];
        let mut utterances = utts_for(&infos, 1);

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "Собеседник 1");
        assert_eq!(infos[1].name, "Собеседник 2");
        assert_eq!(utterances[0].speaker, "Собеседник 1");
        assert_eq!(utterances[1].speaker, "Собеседник 2");
    }

    #[test]
    fn anonymous_tail_is_not_merged() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card(
                "b",
                "Собеседник 2",
                vec![0.9, (1.0f64 - 0.81).sqrt()],
                false,
                0.0,
                0.5,
            ),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "Собеседник 1", 1),
            utterance(100.0, 104.0, "Собеседник 2", 1),
        ];

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "Собеседник 1");
        assert_eq!(infos[1].name, "Собеседник 2");
        assert_eq!(utterances[0].speaker, "Собеседник 1");
        assert_eq!(utterances[1].speaker, "Собеседник 2");
    }

    #[test]
    fn anonymous_group_with_two_small_tails_is_not_merged() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("b", "Собеседник 2", vec![0.9, 0.435_889_9], false, 0.0, 0.5),
            card("c", "Собеседник 3", vec![0.9, 0.435_889_9], false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "Собеседник 1", 1),
            utterance(100.0, 103.0, "Собеседник 2", 1),
            utterance(103.0, 105.0, "Собеседник 3", 1),
        ];

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 3);
    }

    #[test]
    fn anonymous_group_with_three_min_tails_is_not_merged() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("b", "Собеседник 2", vec![0.9, 0.435_889_9], false, 0.0, 0.5),
            card("c", "Собеседник 3", vec![0.9, 0.435_889_9], false, 0.0, 0.5),
            card("d", "Собеседник 4", vec![0.9, 0.435_889_9], false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "Собеседник 1", 1),
            utterance(100.0, 101.5, "Собеседник 2", 1),
            utterance(101.5, 103.0, "Собеседник 3", 1),
            utterance(103.0, 104.5, "Собеседник 4", 1),
        ];

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 4);
    }

    #[test]
    fn anonymous_group_with_five_second_tail_is_not_merged() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("b", "Собеседник 2", vec![0.9, 0.435_889_9], false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "Собеседник 1", 1),
            utterance(100.0, 105.0, "Собеседник 2", 1),
        ];

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 2);
    }

    #[test]
    fn all_matched_group_without_confident_name_gets_a_new_anonymous_name() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5),
            card("b", "B", vec![0.8, 0.6], true, 0.52, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert!(info.name.starts_with("Собеседник "));
        assert_ne!(info.name, "A");
        assert_ne!(info.name, "B");
        assert!(utterances.iter().all(|u| u.speaker == info.name));
        assert_eq!(info.samples.len(), 2);
    }

    #[test]
    fn fresh_anonymous_name_in_stereo_mic_uses_uchastnik_prefix() {
        let mut first = card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5);
        first.channel = Some(0);
        let mut second = card("b", "B", vec![0.8, 0.6], true, 0.52, 0.5);
        second.channel = Some(0);
        let mut infos = vec![first, second];
        let mut utterances = utts_for(&infos, 0);

        merge_verified_cards(&mut infos, &mut utterances, true);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert!(info.name.starts_with("Участник "));
        assert!(utterances.iter().all(|u| u.speaker == info.name));
        assert_eq!(info.samples.len(), 2);
    }

    #[test]
    fn duplicate_names_are_not_merged() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("b", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card("c", "A", vec![1.0, 0.0], true, 0.80, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 3);
        assert_eq!(
            infos
                .iter()
                .map(|info| info.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Собеседник 1", "Собеседник 1", "A"]
        );
        assert_eq!(utterances[0].speaker, "Собеседник 1");
        assert_eq!(utterances[1].speaker, "Собеседник 1");
        assert_eq!(utterances[2].speaker, "A");
    }

    #[test]
    fn single_weak_name_without_anonymous_is_kept() {
        let mut first = card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5);
        first.matched_name = Some("A".to_string());
        let mut second = card("b", "B", vec![0.8, 0.6], true, 0.51, 0.5);
        second.matched_name = Some("A".to_string());
        let mut infos = vec![first, second];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert_eq!(info.name, "A");
        assert!(info.matched);
        assert_eq!(info.similarity, 0.51);
        assert!(utterances.iter().all(|u| u.speaker == "A"));
    }

    #[test]
    fn weak_name_keeps_short_anonymous_tail() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5),
            card("b", "Собеседник 1", vec![0.8, 0.6], false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "A", 1),
            utterance(100.0, 101.6, "Собеседник 1", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert_eq!(info.name, "A");
        assert!(info.matched);
        assert_eq!(info.similarity, 0.51);
        assert!(utterances.iter().all(|u| u.speaker == "A"));
    }

    #[test]
    fn substantial_anonymous_group_demotes_to_largest_anonymous_target() {
        let mut infos = vec![
            card("a", "A", sim_vector(0, 3, 0.8), true, 0.51, 0.5),
            card("b", "Собеседник 1", sim_vector(1, 3, 0.8), false, 0.0, 0.5),
            card("c", "Собеседник 2", sim_vector(2, 3, 0.8), false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 10.0, "A", 1),
            utterance(10.0, 50.0, "Собеседник 1", 1),
            utterance(50.0, 55.0, "Собеседник 2", 1),
        ];

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert_eq!(info.name, "Собеседник 1");
        assert!(utterances.iter().all(|u| u.speaker == "Собеседник 1"));
    }

    #[test]
    fn weak_name_does_not_absorb_anonymous_voice() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5),
            card("b", "Собеседник 1", vec![0.8, 0.6], false, 0.0, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert_eq!(info.name, "Собеседник 1");
        assert!(utterances.iter().all(|u| u.speaker == "Собеседник 1"));
    }

    #[test]
    fn fresh_name_does_not_reuse_absorbed_profile_name() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], true, 0.51, 0.5),
            card("b", "A", vec![0.8, 0.6], true, 0.52, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert_eq!(info.name, "Собеседник 2");
        assert!(utterances.iter().all(|u| u.speaker == "Собеседник 2"));
        assert_eq!(info.samples.len(), 2);
    }

    #[test]
    fn fresh_name_does_not_reuse_candidate_profile_name() {
        let mut first = card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5);
        first.candidates = vec![("Собеседник 1".to_string(), 0.9), ("A".to_string(), 0.51)];
        let second = card("b", "B", vec![0.8, 0.6], true, 0.52, 0.5);
        let mut infos = vec![first, second];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert_eq!(info.name, "Собеседник 2");
        assert!(utterances.iter().all(|u| u.speaker == "Собеседник 2"));
    }

    #[test]
    fn fresh_name_excludes_matched_names() {
        let mut first = card("a", "B", vec![1.0, 0.0], true, 0.51, 0.5);
        first.matched_name = Some("Собеседник 1".to_string());
        first.candidates.clear();
        let mut second = card("b", "D", vec![0.8, 0.6], true, 0.51, 0.5);
        second.matched_name = Some("Собеседник 2".to_string());
        second.candidates.clear();
        let mut infos = vec![first, second];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert_eq!(info.name, "Собеседник 3");
        assert!(utterances.iter().all(|u| u.speaker == "Собеседник 3"));
        assert_eq!(info.samples.len(), 2);
    }

    #[test]
    fn named_target_collision_with_outside_card_skips_group() {
        let mut infos = vec![
            card("a", "A", vec![0.0, 1.0], false, 0.0, 0.5),
            card("b", "B", vec![1.0, 0.0], true, 0.80, 0.5),
            card("c", "C", vec![1.0, 0.0], true, 0.80, 0.5),
        ];
        infos[1].matched_name = Some("A".to_string());
        infos[2].matched_name = Some("A".to_string());
        let mut utterances = utts_for(&infos, 1);

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].name, "A");
        assert_eq!(infos[1].name, "B");
        assert_eq!(infos[2].name, "C");
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(utterances[1].speaker, "B");
        assert_eq!(utterances[2].speaker, "C");
    }

    #[test]
    fn ineligible_duplicate_name_does_not_block_merge() {
        let eligible = card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5);
        let mut missing = card("b", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5);
        missing.embedding = None;
        let mut infos = vec![
            eligible,
            missing,
            card("c", "A", vec![1.0, 0.0], true, 0.80, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 10.0, "Собеседник 1", 1),
            utterance(10.0, 20.0, "A", 1),
        ];

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(merged);
        assert_eq!(infos.len(), 2);
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(utterances[1].speaker, "A");
    }

    #[test]
    fn multiple_groups_and_middle_removal() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0, 0.0], true, 0.80, 0.5),
            card("b", "Собеседник 2", vec![1.0, 0.0, 0.0], false, 0.0, 0.5),
            card("c", "Собеседник 3", vec![0.0, 1.0, 0.0], false, 0.0, 0.5),
            card("d", "B", vec![0.0, 0.0, 1.0], true, 0.80, 0.5),
            card("e", "Собеседник 5", vec![0.0, 0.0, 1.0], false, 0.0, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(merged);
        assert_eq!(infos.len(), 3);
        assert_eq!(
            infos
                .iter()
                .map(|info| info.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "Собеседник 3", "B"]
        );
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(utterances[1].speaker, "A");
        assert_eq!(utterances[2].speaker, "Собеседник 3");
        assert_eq!(utterances[3].speaker, "B");
        assert_eq!(utterances[4].speaker, "B");
    }

    #[test]
    fn matched_pack_fields_are_cleared_on_demotion() {
        let mut first = card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5);
        first.matched_pack_id = Some("pack-a".to_string());
        first.matched_pack_name = Some("Pack A".to_string());
        let mut second = card("b", "B", vec![0.8, 0.6], true, 0.52, 0.5);
        second.matched_pack_id = Some("pack-b".to_string());
        second.matched_pack_name = Some("Pack B".to_string());
        let mut infos = vec![first, second];
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert!(info.matched_name.is_none());
        assert!(info.matched_pack_id.is_none());
        assert!(info.matched_pack_name.is_none());
        assert_eq!(info.similarity, 0.0);
        assert!(utterances.iter().all(|u| u.speaker == info.name));
    }

    #[test]
    fn merged_card_channel_uses_dominant_utterance_channel() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.80, 0.5),
            card("b", "Собеседник 1", vec![0.8, 0.6], false, 0.0, 0.5),
        ];
        infos[0].channel = Some(0);
        let mut utterances = utts_for(&infos, 1);

        merge_verified_cards(&mut infos, &mut utterances, false);

        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].channel, Some(1));

        let mut mono_infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.80, 0.5),
            card("b", "Собеседник 1", vec![0.8, 0.6], false, 0.0, 0.5),
        ];
        mono_infos[0].channel = None;
        mono_infos[1].channel = None;
        let mut mono_utterances = utts_for(&mono_infos, 0);
        for utterance in &mut mono_utterances {
            utterance.channel = None;
        }

        merge_verified_cards(&mut mono_infos, &mut mono_utterances, false);

        assert_eq!(mono_infos.len(), 1);
        assert_eq!(mono_infos[0].channel, None);
    }

    #[test]
    fn card_without_embedding_is_not_merged() {
        let mut missing = card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5);
        missing.embedding = None;
        let mut infos = vec![
            missing,
            card("b", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(!merged);
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "A");
        assert_eq!(infos[1].name, "Собеседник 1");
        assert_eq!(utterances[0].speaker, "A");
        assert_eq!(utterances[1].speaker, "Собеседник 1");
    }

    #[test]
    fn fresh_anonymous_name_renames_target_utterances() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.51, 0.5),
            card("b", "B", vec![0.8, 0.6], true, 0.52, 0.5),
        ];
        let mut utterances = utts_for(&infos, 1);

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(merged);
        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert!(utterances.iter().all(|u| u.speaker == info.name));
        assert_eq!(info.samples.len(), 2);
    }

    #[test]
    fn large_anonymous_card_absorbs_weak_fragment() {
        let mut infos = vec![
            card("a", "Собеседник 1", vec![1.0, 0.0], false, 0.0, 0.5),
            card(
                "b",
                "A",
                vec![0.86, (1.0f64 - 0.86 * 0.86).sqrt()],
                true,
                0.48,
                0.5,
            ),
        ];
        let mut utterances = vec![
            utterance(0.0, 100.0, "Собеседник 1", 1),
            utterance(100.0, 107.0, "A", 1),
        ];

        let merged = merge_verified_cards(&mut infos, &mut utterances, false);

        assert!(merged);
        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.matched);
        assert_eq!(info.name, "Собеседник 1");
        assert_eq!(utterances.len(), 2);
        assert!(utterances.iter().all(|u| u.speaker == info.name));
    }

    #[test]
    fn merge_then_finalize_and_refresh_has_no_ghost_speaker() {
        let mut infos = vec![
            card("a", "A", vec![1.0, 0.0], true, 0.80, 0.5),
            card("b", "Собеседник 1", vec![0.8, 0.6], false, 0.0, 0.5),
        ];
        let mut utterances = vec![
            utterance(0.0, 10.0, "A", 1),
            utterance(10.0, 20.0, "Собеседник 1", 1),
        ];
        assert!(merge_verified_cards(&mut infos, &mut utterances, false));

        let channels = HashMap::new();
        crate::transcribe::profile_verification::finalize_verified_utterances(
            &mut utterances,
            &mut infos,
            &channels,
        );
        super::super::refresh_speaker_stats(&mut infos, &utterances);

        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "A");
        assert_eq!(infos[0].utterances_count, utterances.len());
        assert!(!infos[0].samples.is_empty());
        assert!(utterances.iter().all(|u| u.speaker == "A"));
        assert!(utterances
            .iter()
            .all(|u| infos.iter().any(|info| info.name == u.speaker)));
    }
}
