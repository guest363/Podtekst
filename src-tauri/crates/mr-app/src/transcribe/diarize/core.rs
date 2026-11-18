use std::collections::{HashMap, HashSet};

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::segmentation::Segmentation;
use mr_ml::{match_voiceprints_matrix, ClusterMatch, SpeakerProfile};

use super::constraint_links::{cannot_link_units, pairs_above_sim};
use super::profiles::{eligible_clusters, empty_match, pick_samples, reassign_short_replies};
use super::segments::{
    constraint_retry_needed, fallback_info, fill_labels_by_time, fill_unit_labels_by_time,
    log_cluster_summary, merge_tiny_clusters, promote_unmatched_utterances, promoted_utterances,
    short_join_threshold, smooth_labels_mono,
};
use super::{acoustic_units, coverage, diagnostics, turn_anchors};
use super::{
    CORE_LINKAGE_THRESHOLD, CORE_MIN_DUR, MIN_NAMED_CLEAN_SEC, SHORT_NAME_MIN_MARGIN,
    SHORT_NAME_MIN_SIM,
};

use crate::transcribe::clean_speech::SpeechSupport;
use crate::transcribe::echo::EchoEvidence;
use crate::transcribe::experiment::{self, Anchors, Centroids, Guards};
use crate::transcribe::word_speakers::resolve_word_bounds;
use crate::transcribe::{SpeakerInfo, Utterance, WordConf};

pub(super) fn short_name_allowed(candidate: &ClusterMatch) -> bool {
    candidate.matched
        && candidate.similarity >= SHORT_NAME_MIN_SIM
        && candidate.margin >= SHORT_NAME_MIN_MARGIN
}

/// Верхняя граница auto-k, если пользователь не задал max_speakers: защита от
/// дробления шума на десятки «голосов». shortcut: 12 без размеченного набора.
const AUTO_K_MAX_SPEAKERS: usize = 12;
/// Минимальная длительность реплики `ch=1` для доразбиения по чистым прогонам
/// `PyAnnote`: короткие фрагменты уже покрываются разрезанием по паузам.
const REFINE_MIN_UTTERANCE_SEC: f64 = 6.0;
/// Чистый прогон и оба отступа от краёв реплики не короче 1 с, иначе
/// случайные всплески или края дают рваные обрывки.
const REFINE_MIN_RUN_SEC: f64 = 1.0;
const REFINE_MIN_PAUSE_SEC: f64 = 0.12;
/// Допуск между краем прогона `PyAnnote` и границей слова: учитывает окно
/// сглаживания сегментации и отброшенные кадры перекрытия перед речью.
const REFINE_ALIGN_TOLERANCE_SEC: f64 = 0.5;
/// Склейка соседних односпикерных прогонов через короткое перекрытие/тишину,
/// чтобы один внутренний стык не дробил непрерывный ответ собеседника.
const REFINE_RUN_COALESCE_GAP_SEC: f64 = 0.5;
/// Калибровка слияния центроидов (ERes2NetV2): разные дикторы ≤ 0.22,
/// один голос ≥ 0.48 на фрагментах. Порог 0.70 предотвращает ложное
/// слияние разных собеседников с похожей акустикой комнаты.
const CENTROID_MERGE_SIM: f64 = 0.70;

/// Замер 2026-11-13 (runs/crosslink-diag2, crosslink-esc2): пары схлопывающихся
/// синтетических сценариев — sim 0.55–0.68, пары main-набора — 0.13–0.33.
/// В первом прогоне активны только пары, противоречащие близости эмбеддингов
/// (локальное доказательство против глобального слияния); полный набор —
/// только при ретрае после коллапса в один кластер.
const CROSS_TURN_ACTIVE_SIM: f64 = 0.5;

#[allow(clippy::too_many_arguments)]
pub(crate) fn diarize_channel(
    samples: &[f32],
    utterances: &mut Vec<Utterance>,
    channel_idx: Option<i32>,
    default_name: &str,
    prefix: &str,
    extractor: &mut Option<EmbeddingExtractor>,
    segmentation: Option<&Segmentation>,
    vad_probs: Option<&[f32]>,
    echo: Option<&EchoEvidence>,
    profiles: &[SpeakerProfile],
    min_speakers: usize,
    max_speakers: usize,
    on_event: &dyn Fn(u8, &str),
) -> (Vec<SpeakerInfo>, Vec<Utterance>) {
    use mr_ml::cluster::{agglomerative_gap, agglomerative_weighted, merge_by_centroid_cosine};
    use mr_ml::cosine_similarity;
    let experiment = experiment::current();
    // EchoEvidence описывает пролезание системы в микрофон (ch=0); к другим каналам неприменим.
    let echo = echo.filter(|_| channel_idx == Some(0));
    let support = SpeechSupport {
        vad_probs,
        segmentation,
    };
    let mut n = utterances.len();
    if n == 0 {
        return (vec![], Vec::new());
    }
    let ch = channel_idx.unwrap_or(-1);
    log::debug!(
        target: "diar",
        ch = ch,
        utts = n,
        embed = extractor.is_some(),
        segmentation = segmentation.is_some(),
        min_spk = min_speakers,
        max_spk = max_speakers;
        "ev=diar.start"
    );
    let mut embs: Vec<Option<Vec<f64>>> = vec![None; n];
    let mut durs = vec![0.0; n];
    let mut quals = vec![1.0f64; n];
    let acoustic_units =
        acoustic_units::build(samples, utterances, extractor, support, echo, on_event);
    let anchors = acoustic_units.anchors;
    let mut units = acoustic_units.items;
    if units.is_empty() {
        for u in utterances.iter_mut() {
            u.speaker = default_name.to_string();
            u.diarized = true;
        }
        return (
            vec![fallback_info(default_name, channel_idx, utterances)],
            Vec::new(),
        );
    }
    let mut core_units = match experiment.anchors {
        Anchors::Turns => turn_anchors::core_indices(&units, &anchors),
        Anchors::Windows => {
            let core: Vec<_> = (0..units.len())
                .filter(|&index| units[index].dur >= CORE_MIN_DUR)
                .collect();
            if core.is_empty() {
                (0..units.len()).collect()
            } else {
                core
            }
        }
    };
    let (dur_min, dur_max) = core_units
        .iter()
        .fold((f64::INFINITY, 0.0f64), |(min, max), &index| {
            (min.min(units[index].dur), max.max(units[index].dur))
        });
    log::debug!(
        target: "diar",
        ch = ch,
        utts = n,
        units = units.len(),
        anchors = anchors.len(),
        core_units = core_units.len(),
        dur_min = if dur_min.is_finite() { dur_min } else { 0.0 },
        dur_max = dur_max;
        "ev=diar.units"
    );
    let core_geometry = |core_units: &[usize]| -> (Vec<Vec<f64>>, Vec<f64>) {
        (
            core_units
                .iter()
                .map(|&index| units[index].embedding.clone())
                .collect(),
            core_units
                .iter()
                .map(|&index| units[index].dur.clamp(0.4, 4.0) * units[index].quality)
                .collect(),
        )
    };
    let (mut core_vecs, mut core_weights) = core_geometry(&core_units);
    let k = if min_speakers > 0 && min_speakers == max_speakers {
        Some(min_speakers)
    } else {
        None
    };
    // Калибровка: авто-режим допускает монологи 1..max_speakers без принуждения к 2 спикерам.
    let min_auto = if min_speakers == 0 { 1 } else { min_speakers };
    let core_pos: HashMap<usize, usize> = core_units
        .iter()
        .enumerate()
        .map(|(position, &index)| (index, position))
        .collect();
    let pairs = cannot_link_units(&units, &core_pos, support, &core_vecs);
    let auto_max = if max_speakers > 0 {
        max_speakers
    } else {
        AUTO_K_MAX_SPEAKERS
    };
    let cluster_core =
        |core_vecs: &[Vec<f64>], core_weights: &[f64], constraints: &[(usize, usize)]| {
            if let Some(k) = k {
                agglomerative_weighted(
                    core_vecs,
                    core_weights,
                    constraints,
                    CORE_LINKAGE_THRESHOLD,
                    Some(k),
                )
            } else {
                agglomerative_gap(
                    core_vecs,
                    core_weights,
                    constraints,
                    CORE_LINKAGE_THRESHOLD,
                    min_auto,
                    auto_max,
                )
            }
        };
    // Замер 2026-11-12 (runs/crosslink-diag2, crosslink-esc): постоянные
    // cannot-link искажают выбор k даже при верных парах (main-en-quiet: 2 → 3),
    // а без них конвейер сваливается в один кластер в reject_small_clusters
    // (synth-ru-2spk: 2 → 1). Первый прогон — только горячие пары
    // (sim ≥ CROSS_TURN_ACTIVE_SIM); полный набор — при ретрае после k=1.
    let initial_core = core_units.clone();
    let mut use_full = k.is_some();
    let mut constraints = if use_full {
        pairs.clone()
    } else {
        pairs_above_sim(&core_vecs, &pairs, CROSS_TURN_ACTIVE_SIM)
    };
    let mut core_labels = cluster_core(&core_vecs, &core_weights, &constraints);
    let mut after_ahc = 0;
    for attempt in 0..2 {
        if attempt > 0 {
            core_units = initial_core.clone();
            (core_vecs, core_weights) = core_geometry(&core_units);
            constraints = if use_full {
                pairs.clone()
            } else {
                pairs_above_sim(&core_vecs, &pairs, CROSS_TURN_ACTIVE_SIM)
            };
            core_labels = cluster_core(&core_vecs, &core_weights, &constraints);
        }
        let candidate_k = core_labels.iter().copied().collect::<HashSet<_>>().len();
        diagnostics::core("ahc", ch, &units, &core_units, &core_labels, support);
        if let Some(requested) = k {
            log::debug!(
                target: "diar",
                ch = ch,
                requested_k = requested,
                actual_k = candidate_k,
                coverage_enabled = false,
                merge_enabled = false,
                smoothing_enabled = min_speakers == 0;
                "ev=diar.fixed_k"
            );
        }
        if k.is_none() && experiment.guards == Guards::On {
            let validated = coverage::reject_small_clusters(
                &units,
                &core_units,
                core_labels,
                &core_weights,
                &constraints,
                support,
                min_auto..=auto_max,
            );
            core_units = validated.core_units;
            core_labels = validated.labels;
            (core_vecs, core_weights) = core_geometry(&core_units);
            constraints = validated.constraints;
        }
        after_ahc = core_labels.iter().copied().collect::<HashSet<_>>().len();
        diagnostics::core("coverage", ch, &units, &core_units, &core_labels, support);
        // Короткие шумные куски часто образуют ложные «голоса». Сливаем их с
        // ближайшим похожим голосом (косинус ≥ 0.45) как в моно, так и в стерео.
        if k.is_none() {
            let weights: Vec<f64> = core_units.iter().map(|&index| units[index].dur).collect();
            merge_tiny_clusters(
                &core_vecs,
                &mut core_labels,
                &weights,
                min_auto,
                profiles,
                &constraints,
            );
        }
        diagnostics::core("tiny_merge", ch, &units, &core_units, &core_labels, support);
        if k.is_none() {
            let mut uniq: Vec<usize> = core_labels.clone();
            uniq.sort();
            uniq.dedup();
            if max_speakers > 0 && uniq.len() > max_speakers {
                log::debug!(
                    target: "diar",
                    ch = ch,
                    before = uniq.len(),
                    after = max_speakers,
                    max_spk = max_speakers;
                    "ev=diar.max_speakers"
                );
                core_labels = agglomerative_weighted(
                    &core_vecs,
                    &core_weights,
                    &constraints,
                    f64::INFINITY,
                    Some(max_speakers),
                );
            } else if min_speakers > 1
                && uniq.len() < min_speakers
                && core_vecs.len() >= min_speakers
            {
                log::debug!(
                    target: "diar",
                    ch = ch,
                    before = uniq.len(),
                    after = min_speakers,
                    min_spk = min_speakers;
                    "ev=diar.max_speakers"
                );
                core_labels = agglomerative_weighted(
                    &core_vecs,
                    &core_weights,
                    &constraints,
                    f64::INFINITY,
                    Some(min_speakers),
                );
            }
        }
        if k.is_none() {
            merge_by_centroid_cosine(
                &core_vecs,
                &core_weights,
                &mut core_labels,
                &constraints,
                CENTROID_MERGE_SIM,
                min_auto,
            );
        }
        if k.is_none() && experiment.guards == Guards::On {
            let validated = coverage::reject_small_clusters(
                &units,
                &core_units,
                core_labels,
                &core_weights,
                &constraints,
                support,
                min_auto..=auto_max,
            );
            core_units = validated.core_units;
            core_labels = validated.labels;
            (core_vecs, core_weights) = core_geometry(&core_units);
            constraints = validated.constraints;
        }
        diagnostics::core("validated", ch, &units, &core_units, &core_labels, support);
        log_cluster_summary(ch, k, &core_vecs, &core_labels, constraints.len());
        if constraint_retry_needed(
            k.is_some(),
            use_full,
            core_labels.iter().copied().collect::<HashSet<_>>().len(),
            !pairs.is_empty(),
        ) {
            log::debug!(
                target: "diar",
                ch = ch,
                pairs = pairs.len();
                "ev=diar.constraint_escalation"
            );
            use_full = true;
            continue;
        }
        break;
    }
    on_event(88, "Голоса разделены по спикерам");
    let mut unit_label: HashMap<usize, usize> = core_units
        .iter()
        .copied()
        .zip(core_labels.iter().copied())
        .collect();
    let recompute_units = |label_of: &HashMap<usize, usize>| -> HashMap<usize, Vec<f64>> {
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for (&index, &label) in label_of {
            groups.entry(label).or_default().push(index);
        }
        groups
            .into_iter()
            .map(|(label, members)| {
                let vecs: Vec<Vec<f64>> = members
                    .iter()
                    .map(|&index| units[index].embedding.clone())
                    .collect();
                let ws: Vec<f64> = members
                    .iter()
                    .map(|&index| units[index].dur.clamp(0.4, 4.0) * units[index].quality)
                    .collect();
                (label, mr_ml::weighted_centroid(&vecs, &ws))
            })
            .collect()
    };
    let mut unit_centroids = recompute_units(&unit_label);
    for (index, unit) in units.iter().enumerate() {
        if unit_label.contains_key(&index) {
            continue;
        }
        let scored = |skip: Option<usize>| {
            unit_centroids
                .iter()
                .filter(|(&label, _)| Some(label) != skip)
                .map(|(&label, centroid)| (label, cosine_similarity(&unit.embedding, centroid)))
                // Тай-брейк по номеру кластера: обход `HashMap` случаен.
                .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        };
        let Some((best_label, best_score)) = scored(None) else {
            continue;
        };
        let second_score = scored(Some(best_label))
            .map(|(_, score)| score)
            .unwrap_or(f64::NEG_INFINITY);
        let threshold = short_join_threshold(unit.dur);
        log::debug!(
            target: "diar",
            ch = ch,
            i = index,
            top1_id = best_label as i64,
            top1_sim = best_score,
            top2_sim = if second_score.is_finite() { second_score } else { 0.0 },
            margin = if second_score.is_finite() { best_score - second_score } else { best_score },
            threshold = threshold,
            matched = best_score >= threshold;
            "ev=diar.assign"
        );
        if best_score >= threshold {
            unit_label.insert(index, best_label);
            if experiment.centroids == Centroids::Online {
                unit_centroids = recompute_units(&unit_label);
            }
        }
    }
    let mut next_label = unit_label
        .values()
        .copied()
        .max()
        .map_or(0, |label| label + 1);
    let promoted: HashSet<usize> = promote_unmatched_utterances(
        &units,
        &mut unit_label,
        &mut next_label,
        auto_max,
        support,
        echo,
    )
    .into_iter()
    .collect();
    if !promoted.is_empty() {
        log::debug!(
            target: "diar",
            ch = ch,
            promoted = promoted.len();
            "ev=diar.promote_unknown"
        );
    }
    fill_unit_labels_by_time(&units, &mut unit_label);
    if experiment.anchors == Anchors::Turns {
        turn_anchors::retain_refinement_labels(&units, &anchors, &mut unit_label);
    }
    let protected = match experiment.guards {
        Guards::On => {
            coverage::split_covered_utterances(utterances, &mut units, &mut unit_label, support)
        }
        Guards::Off => HashMap::new(),
    };
    n = utterances.len();
    embs.resize(n, None);
    durs.resize(n, 0.0);
    quals.resize(n, 1.0);
    let label_duration = coverage::label_durations(&units, &unit_label, support);
    let mut durations_by_utterance: HashMap<usize, Vec<(usize, f64)>> = HashMap::new();
    for (&(utterance, label), &duration) in &label_duration {
        durations_by_utterance
            .entry(utterance)
            .or_default()
            .push((label, duration));
    }
    let mut units_by_utterance: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (index, unit) in units.iter().enumerate() {
        if unit.utterance < n {
            units_by_utterance[unit.utterance].push(index);
        }
    }
    let mut label_of: HashMap<usize, usize> = HashMap::new();
    let mut valid: Vec<usize> = vec![];
    for utterance in 0..n {
        let Some((label, _)) = durations_by_utterance
            .get(&utterance)
            .and_then(|durations| {
                durations
                    .iter()
                    .copied()
                    .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            })
        else {
            continue;
        };
        let members: Vec<usize> = units_by_utterance[utterance]
            .iter()
            .copied()
            .filter(|&index| unit_label.get(&index) == Some(&label))
            .collect();
        if members.is_empty() {
            continue;
        }
        let vecs: Vec<Vec<f64>> = members
            .iter()
            .map(|&index| units[index].embedding.clone())
            .collect();
        let mut intervals = Vec::with_capacity(members.len());
        let mut covered = 0.0;
        let ws: Vec<f64> = members
            .iter()
            .map(|&index| {
                let unit = &units[index];
                intervals.push((unit.start, unit.end));
                let total = support.duration(&intervals);
                let weight = (total - covered).max(0.0) * unit.quality;
                covered = total;
                weight
            })
            .collect();
        let total_dur: f64 = members.iter().map(|&index| units[index].dur).sum();
        let quality: f64 = members
            .iter()
            .map(|&index| units[index].quality * units[index].dur)
            .sum::<f64>()
            / total_dur.max(1e-6);
        embs[utterance] = Some(mr_ml::weighted_centroid(&vecs, &ws));
        durs[utterance] = covered;
        quals[utterance] = quality;
        label_of.insert(utterance, label);
        valid.push(utterance);
    }
    if valid.is_empty() {
        for u in utterances.iter_mut() {
            u.speaker = default_name.to_string();
            u.diarized = true;
        }
        return (
            vec![fallback_info(default_name, channel_idx, utterances)],
            Vec::new(),
        );
    }
    // Индексы реплик уже пересобраны split_covered_utterances:
    // неизвестные голоса ищем по меткам, а не по индексам до split.
    let promoted_utts = promoted_utterances(&label_of, &promoted);
    fill_labels_by_time(utterances, &mut label_of);
    let after_coverage = label_of.values().copied().collect::<HashSet<_>>().len();
    diagnostics::turns("assignment", ch, utterances, &label_of, support);

    // Одиночный шумный кусок не должен рвать монолог на два голоса.
    // При явно заданном минимуме спикеров не сглаживаем: метки важнее.
    if channel_idx != Some(0) && min_speakers == 0 {
        smooth_labels_mono(utterances, &embs, &durs, &quals, &mut label_of);
        // На ch=1 одиночные реплики неизвестного собеседника не должны
        // поглощаться соседом: дальнейшее уточнение делает speaker_assignment.
        if channel_idx == Some(1) {
            coverage::restore_protected(&mut label_of, &promoted_utts);
        }
    }
    let after_smooth = label_of.values().copied().collect::<HashSet<_>>().len();
    diagnostics::turns("smoothing", ch, utterances, &label_of, support);
    coverage::restore_protected(&mut label_of, &protected);
    let mut voice_runs: Vec<crate::transcribe::speaker_assignment::SpeakerSpan> = Vec::new();
    let promoted_keys: HashSet<usize> = promoted_utts.keys().copied().collect();
    if let Some(segmentation) = segmentation {
        let outcome = crate::transcribe::speaker_assignment::apply(
            segmentation,
            utterances,
            &embs,
            &durs,
            &quals,
            &mut label_of,
            &promoted_keys,
            max_speakers,
        );
        // Поле spans нужно для unit-тестов speaker_assignment; доразбиение опирается на voice_runs.
        let _ = outcome.spans;
        voice_runs = outcome.voice_runs;
    }
    let after_assignment = label_of.values().copied().collect::<HashSet<_>>().len();
    coverage::restore_protected(&mut label_of, &protected);
    if echo.is_some() {
        reassign_short_replies(utterances, &embs, &durs, &quals, &mut label_of);
    }
    coverage::restore_protected(&mut label_of, &protected);
    let final_vecs: Vec<Vec<f64>> = valid.iter().map(|&i| embs[i].clone().unwrap()).collect();
    diagnostics::turns("final", ch, utterances, &label_of, support);
    let final_labels: Vec<usize> = valid.iter().map(|&i| label_of[&i]).collect();
    let final_scores = mr_ml::cluster::silhouette_scores(&final_vecs, &final_labels);
    let mut silhouettes = vec![0.0f64; n];
    for (&i, score) in valid.iter().zip(final_scores) {
        silhouettes[i] = score;
    }
    let mut raw_clusters: Vec<usize> = label_of.values().copied().collect();
    raw_clusters.sort();
    raw_clusters.dedup();
    let candidate_k = core_labels.iter().copied().collect::<HashSet<_>>().len();
    if candidate_k >= 2 && raw_clusters.len() == 1 {
        log::debug!(target: "diar", ch = ch, before = candidate_k, after_ahc = after_ahc, after_coverage = after_coverage, after_smooth = after_smooth, after_assignment = after_assignment, after = 1; "ev=diar.collapse");
    }
    let cluster_centroid = |c: usize| -> Vec<f64> {
        let members: Vec<usize> = {
            let mut members: Vec<usize> = label_of
                .iter()
                .filter(|(_, &l)| l == c)
                .map(|(&i, _)| i)
                .collect();
            // Порядок обхода HashMap не должен влиять на сумму центроида.
            members.sort_unstable();
            members
        };
        // Пары «вектор-вес» строим вместе: иначе веса съезжают на
        // реплики без эмбеддинга и центроид считается не по тем голосам.
        let weighted: Vec<(Vec<f64>, f64)> = members
            .iter()
            .filter_map(|&i| {
                embs[i]
                    .clone()
                    .map(|vector| (vector, durs[i].min(8.0) * quals[i]))
            })
            .collect();
        if weighted.is_empty() {
            vec![0.0; mr_ml::EMBEDDING_DIM]
        } else {
            let (vecs, ws): (Vec<Vec<f64>>, Vec<f64>) = weighted.into_iter().unzip();
            mr_ml::weighted_centroid(&vecs, &ws)
        }
    };
    let eligible = eligible_clusters(&raw_clusters, &label_of, &embs, &durs);
    let (long, short): (Vec<_>, Vec<_>) = eligible
        .into_iter()
        .partition(|&(_, clean_sec)| clean_sec >= MIN_NAMED_CLEAN_SEC);
    let mut matches: Vec<ClusterMatch> = raw_clusters.iter().map(|_| empty_match()).collect();
    // Длинные кластеры матчатся первыми и не конкурируют с коротким образцом:
    // иначе точное совпадение короткой реплики могло занять имя и оставить
    // длинный кластер того же человека безымянным.
    let long_centroids: Vec<Vec<f64>> = long
        .iter()
        .map(|&(k, _)| cluster_centroid(raw_clusters[k]))
        .collect();
    let matched_long = match_voiceprints_matrix(
        &long_centroids,
        profiles,
        mr_ml::DEFAULT_MATCH_THRESHOLD,
        mr_ml::DEFAULT_MIN_MARGIN,
    );
    for (slot, &(k, _)) in long.iter().enumerate() {
        matches[k] = matched_long[slot].clone();
    }
    let short_centroids: Vec<Vec<f64>> = short
        .iter()
        .map(|&(k, _)| cluster_centroid(raw_clusters[k]))
        .collect();
    let matched_short = match_voiceprints_matrix(
        &short_centroids,
        profiles,
        mr_ml::DEFAULT_MATCH_THRESHOLD,
        mr_ml::DEFAULT_MIN_MARGIN,
    );
    for (slot, &(k, _)) in short.iter().enumerate() {
        let candidate = &matched_short[slot];
        if short_name_allowed(candidate) {
            matches[k] = candidate.clone();
        }
    }
    let mut cluster_name: HashMap<usize, String> = HashMap::new();
    let mut cluster_pack_id: HashMap<usize, String> = HashMap::new();
    let mut cluster_pack_name: HashMap<usize, String> = HashMap::new();
    let mut unmatched = vec![];
    for (k, &c) in raw_clusters.iter().enumerate() {
        if matches[k].matched {
            let m_name = matches[k].name.clone().unwrap();
            cluster_name.insert(c, m_name);
            if let Some(pid) = &matches[k].pack_id {
                cluster_pack_id.insert(c, pid.clone());
            }
            if let Some(pname) = &matches[k].pack_name {
                cluster_pack_name.insert(c, pname.clone());
            }
        } else {
            unmatched.push(c);
        }
    }
    unmatched.sort_by(|&a, &b| {
        let ca = label_of.values().filter(|&&l| l == a).count();
        let cb = label_of.values().filter(|&&l| l == b).count();
        cb.cmp(&ca).then(
            label_of
                .iter()
                .filter(|(_, &l)| l == b)
                .map(|(&i, _)| utterances[i].start)
                .fold(f64::INFINITY, f64::min)
                .partial_cmp(
                    &label_of
                        .iter()
                        .filter(|(_, &l)| l == a)
                        .map(|(&i, _)| utterances[i].start)
                        .fold(f64::INFINITY, f64::min),
                )
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    let generic = [
        "Собеседник",
        "Собеседники",
        "Участник",
        "Участники",
        "Other",
    ];
    if raw_clusters.len() == 1 && unmatched.len() == 1 {
        cluster_name.insert(raw_clusters[0], default_name.to_string());
    } else {
        let mut used: std::collections::HashSet<String> = cluster_name.values().cloned().collect();
        let default_used = used.contains(default_name) || !cluster_name.is_empty();
        let mut next_num = 1usize;
        for (idx, c) in unmatched.into_iter().enumerate() {
            let name = if idx == 0 && !default_used && !generic.contains(&default_name) {
                default_name.to_string()
            } else {
                while used.contains(&format!("{prefix} {next_num}")) {
                    next_num += 1;
                }
                let name = format!("{prefix} {next_num}");
                next_num += 1;
                name
            };
            used.insert(name.clone());
            cluster_name.insert(c, name);
        }
    }
    let mut unique: Vec<String> = vec![];
    for c in raw_clusters.iter() {
        let nm = &cluster_name[c];
        if !unique.contains(nm) {
            unique.push(nm.clone());
        }
    }
    let mut infos = vec![];
    for name in unique {
        let cids: Vec<usize> = raw_clusters
            .iter()
            .copied()
            .filter(|c| cluster_name[c] == name)
            .collect();
        let members: Vec<usize> = {
            let mut members: Vec<usize> = label_of
                .iter()
                .filter(|(_, &l)| cids.contains(&l))
                .map(|(&i, _)| i)
                .collect();
            members.sort_unstable();
            members
        };
        if members.is_empty() {
            continue;
        }
        let is_matched = cids.iter().any(|c| {
            let k = raw_clusters
                .iter()
                .position(|x| x == c)
                .expect("c из raw_clusters");
            matches[k].matched && matches[k].name.as_deref() == Some(&name)
        });
        let sim = cids
            .iter()
            .map(|c| {
                matches[raw_clusters
                    .iter()
                    .position(|x| x == c)
                    .expect("c из raw_clusters")]
                .similarity
            })
            .fold(0.0f64, f64::max);
        let mut cands = vec![];
        for c in cids.iter() {
            let k = raw_clusters
                .iter()
                .position(|x| x == c)
                .expect("c из raw_clusters");
            for mc in &matches[k].candidates {
                if !cands.iter().any(|(n, _): &(String, f64)| n == &mc.name) {
                    cands.push((mc.name.clone(), mc.similarity));
                }
            }
        }
        let member_utts: Vec<&Utterance> = members.iter().map(|&i| &utterances[i]).collect();
        let samples = pick_samples(&member_utts, 3);
        let (weighted_sil, sil_weight) =
            members
                .iter()
                .filter(|&&i| embs[i].is_some())
                .fold((0.0, 0.0), |(sum, total), &i| {
                    let weight = durs[i].min(8.0) * quals[i];
                    (sum + silhouettes[i] * weight, total + weight)
                });
        let speaker_silhouette = if sil_weight > 0.0 {
            weighted_sil / sil_weight
        } else {
            0.0
        };
        let emb = {
            let weighted: Vec<(Vec<f64>, f64)> = members
                .iter()
                .filter_map(|&i| {
                    embs[i]
                        .clone()
                        .map(|vector| (vector, durs[i].min(8.0) * quals[i]))
                })
                .collect();
            if weighted.is_empty() {
                None
            } else {
                let (vecs, ws): (Vec<Vec<f64>>, Vec<f64>) = weighted.into_iter().unzip();
                Some(mr_ml::weighted_centroid(&vecs, &ws))
            }
        };
        let spk_id = if channel_idx == Some(0) && name == default_name {
            "speaker_me".to_string()
        } else if let Some(ch) = channel_idx {
            format!("spk_ch{ch}_{name}")
        } else {
            format!("spk_{name}")
        };
        let matched_pack_id = if is_matched {
            cids.iter().find_map(|c| cluster_pack_id.get(c)).cloned()
        } else {
            None
        };
        let matched_pack_name = if is_matched {
            cids.iter().find_map(|c| cluster_pack_name.get(c)).cloned()
        } else {
            None
        };
        infos.push(SpeakerInfo {
            id: spk_id,
            name: name.clone(),
            channel: channel_idx,
            matched: is_matched,
            matched_name: if is_matched { Some(name.clone()) } else { None },
            matched_pack_id,
            matched_pack_name,
            similarity: (sim * 1000.0).round() / 1000.0,
            silhouette: (speaker_silhouette * 1000.0).round() / 1000.0,
            candidates: cands,
            embedding: emb.filter(|v| v.iter().any(|&x| x.abs() > 1e-12)),
            samples,
            utterances_count: members.len(),
            total_speech_sec: (members
                .iter()
                .map(|&i| (utterances[i].end - utterances[i].start).max(0.0))
                .sum::<f64>()
                * 10.0)
                .round()
                / 10.0,
        });
    }
    for (i, u) in utterances.iter_mut().enumerate() {
        u.speaker = cluster_name[&label_of[&i]].clone();
        u.diarized = true;
        u.silhouette = silhouettes[i];
    }
    let voice_timeline: Vec<Utterance> = voice_runs
        .iter()
        .filter_map(|span| {
            cluster_name.get(&span.label).map(|name| Utterance {
                start: span.start,
                end: span.end,
                speaker: name.clone(),
                channel: channel_idx,
                text: String::new(),
                diarized: true,
                silhouette: 0.0,
                words: Vec::new(),
                ctc_used: false,
            })
        })
        .collect();
    // Консервативное доразбиение только для ch=1: моно и микрофон сохраняют прежнюю геометрию.
    if channel_idx == Some(1) {
        refine_ch1_utterances(utterances, &voice_timeline, &protected, &promoted_utts);
    }
    if cfg!(debug_assertions) {
        // Контракт бенчмарка: харнесс считает DER по отрезкам диаризации независимо от ASR-текста.
        let segments: Vec<serde_json::Value> = utterances
            .iter()
            .map(|u| {
                serde_json::json!({
                    "start": u.start,
                    "end": u.end,
                    "speaker": crate::logging::speaker_token(&u.speaker),
                    "channel": channel_idx,
                })
            })
            .collect();
        crate::logging::write_artifact_json(
            &format!("diar/ch{ch}.segments.json"),
            &serde_json::json!({ "segments": segments }),
        );
    }
    (infos, voice_timeline)
}

fn is_lexical_boundary(prev: &str, next: &str) -> bool {
    let trimmed = prev.trim_end_matches(['"', '\'', '»', '”', ')', ']']);
    if trimmed.ends_with(['.', '!', '?', '…']) {
        return true;
    }
    if trimmed != "—" && trimmed.ends_with('—') {
        return true;
    }
    next == "—" || next.starts_with('—')
}

fn coalesce_clean_runs(voice_timeline: &[Utterance]) -> Vec<Utterance> {
    let mut clean: Vec<Utterance> = voice_timeline
        .iter()
        .filter(|run| run.end - run.start >= REFINE_MIN_RUN_SEC && !run.speaker.is_empty())
        .cloned()
        .collect();
    clean.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
    let mut merged: Vec<Utterance> = Vec::with_capacity(clean.len());
    for run in clean {
        if let Some(last) = merged.last_mut() {
            if last.speaker == run.speaker && run.start - last.end < REFINE_RUN_COALESCE_GAP_SEC {
                last.end = last.end.max(run.end);
                continue;
            }
        }
        merged.push(run);
    }
    merged
}

struct WordBoundary {
    index: usize,
    lo: f64,
    hi: f64,
    mid: f64,
}

fn eligible_boundaries(
    utterance: &Utterance,
    words: &[&str],
    word_bounds: &[(f64, f64)],
    has_timed_words: bool,
) -> Vec<WordBoundary> {
    let mut out = Vec::new();
    for i in 0..words.len().saturating_sub(1) {
        if words[i + 1..].iter().all(|&word| word == "—") {
            continue;
        }
        let prev_end = word_bounds[i].1;
        // Тире без собственных таймкодов в CTC растягивается на всю паузу между словами,
        // поэтому реальное окно тишины перед репликой доходит до следующего слова.
        let next_start = if words[i + 1] == "—"
            && utterance
                .words
                .get(i + 1)
                .is_none_or(|word| word.end_sec <= word.start_sec)
            && i + 2 < words.len()
        {
            word_bounds[i + 2].0
        } else {
            word_bounds[i + 1].0
        };
        let is_pause = has_timed_words && (next_start - prev_end) >= REFINE_MIN_PAUSE_SEC;
        let is_lexical = is_lexical_boundary(words[i], words[i + 1]);
        if !is_pause && !is_lexical {
            continue;
        }
        out.push(WordBoundary {
            index: i,
            lo: prev_end.min(next_start),
            hi: prev_end.max(next_start),
            mid: f64::midpoint(prev_end, next_start),
        });
    }
    out
}

fn align_edge_to_boundary(
    edge: f64,
    utterance_start: f64,
    utterance_end: f64,
    has_timed_words: bool,
    boundaries: &[WordBoundary],
) -> Option<(usize, f64)> {
    let interval_dist = |bound: &WordBoundary| {
        if edge < bound.lo {
            bound.lo - edge
        } else if edge > bound.hi {
            edge - bound.hi
        } else {
            0.0
        }
    };
    boundaries
        .iter()
        .filter_map(|bound| {
            let dist = interval_dist(bound);
            if dist > REFINE_ALIGN_TOLERANCE_SEC {
                return None;
            }
            let cut_t = if has_timed_words { bound.mid } else { edge };
            if cut_t < utterance_start + REFINE_MIN_RUN_SEC
                || cut_t > utterance_end - REFINE_MIN_RUN_SEC
            {
                return None;
            }
            Some((bound.index, cut_t, dist, (edge - bound.mid).abs()))
        })
        .min_by(|a, b| {
            a.2.total_cmp(&b.2)
                .then_with(|| a.3.total_cmp(&b.3))
                .then_with(|| a.0.cmp(&b.0))
        })
        .map(|(index, cut_t, _, _)| (index, cut_t))
}

fn dominant_piece_speaker(
    utterance: &Utterance,
    runs: &[Utterance],
    start: f64,
    end: f64,
) -> String {
    let mut own_ov = 0.0f64;
    let mut others: HashMap<&str, f64> = HashMap::new();
    for run in runs {
        let ov = (end.min(run.end) - start.max(run.start)).max(0.0);
        if ov <= 0.0 {
            continue;
        }
        if run.speaker == utterance.speaker {
            own_ov += ov;
        } else {
            *others.entry(run.speaker.as_str()).or_default() += ov;
        }
    }
    let best_other = others
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)));
    match best_other {
        Some((speaker, ov))
            if ov >= REFINE_MIN_RUN_SEC && ov > own_ov && ov >= (end - start) * 0.5 =>
        {
            speaker.to_string()
        }
        _ => utterance.speaker.clone(),
    }
}

fn collect_voice_run_cuts(
    utterance: &Utterance,
    runs: &[Utterance],
    has_timed_words: bool,
    boundaries: &[WordBoundary],
) -> Vec<(usize, f64)> {
    let align = |edge: f64| {
        align_edge_to_boundary(
            edge,
            utterance.start,
            utterance.end,
            has_timed_words,
            boundaries,
        )
    };
    let mut cuts: Vec<(usize, f64)> = Vec::new();
    for run in runs {
        if run.speaker == utterance.speaker {
            continue;
        }
        let ov_start = run.start.max(utterance.start);
        let ov_end = run.end.min(utterance.end);
        if ov_end - ov_start < REFINE_MIN_RUN_SEC {
            continue;
        }
        let start_interior = run.start >= utterance.start + REFINE_MIN_RUN_SEC
            && run.start <= utterance.end - REFINE_MIN_RUN_SEC;
        let end_interior = run.end >= utterance.start + REFINE_MIN_RUN_SEC
            && run.end <= utterance.end - REFINE_MIN_RUN_SEC;
        match (start_interior, end_interior) {
            (true, true) => {
                if let (Some(a), Some(b)) = (align(run.start), align(run.end)) {
                    if b.0 > a.0 && b.1 - a.1 >= REFINE_MIN_RUN_SEC {
                        cuts.push(a);
                        cuts.push(b);
                    }
                }
            }
            (true, false) => cuts.extend(align(run.start)),
            (false, true) => cuts.extend(align(run.end)),
            (false, false) => {}
        }
    }
    cuts.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    let mut spaced: Vec<(usize, f64)> = Vec::new();
    for (word_idx, cut_t) in cuts {
        if spaced.last().is_none_or(|&(prev_idx, prev_t)| {
            word_idx > prev_idx && cut_t - prev_t >= REFINE_MIN_RUN_SEC
        }) {
            spaced.push((word_idx, cut_t));
        }
    }
    spaced
}

fn slice_part_words(
    utterance: &Utterance,
    words: &[&str],
    word_bounds: &[(f64, f64)],
    has_timed_words: bool,
    lo: usize,
    hi: usize,
) -> Vec<WordConf> {
    if !has_timed_words {
        return utterance.words.get(lo..hi).unwrap_or(&[]).to_vec();
    }
    (lo..hi)
        .map(|idx| {
            let (s, e) = word_bounds[idx];
            match utterance.words.get(idx).cloned() {
                Some(mut word) => {
                    if word.end_sec <= word.start_sec {
                        word.start_sec = (s * 1000.0).round() / 1000.0;
                        word.end_sec = (e * 1000.0).round() / 1000.0;
                    }
                    word
                }
                None => WordConf {
                    w: words[idx].to_string(),
                    conf: 1.0,
                    needs_review: false,
                    start_sec: (s * 1000.0).round() / 1000.0,
                    end_sec: (e * 1000.0).round() / 1000.0,
                },
            }
        })
        .collect()
}

fn split_utterance_by_voice_runs(utterance: &Utterance, runs: &[Utterance]) -> Vec<Utterance> {
    if utterance.end - utterance.start < REFINE_MIN_UTTERANCE_SEC {
        return vec![utterance.clone()];
    }
    let words: Vec<&str> = utterance.text.split_whitespace().collect();
    if words.len() < 2 {
        return vec![utterance.clone()];
    }
    let has_timed_words = utterance.words.iter().any(|w| w.end_sec > w.start_sec);
    let word_bounds = resolve_word_bounds(&words, &utterance.words, utterance.start, utterance.end);
    let boundaries = eligible_boundaries(utterance, &words, &word_bounds, has_timed_words);
    let spaced = if boundaries.is_empty() {
        Vec::new()
    } else {
        collect_voice_run_cuts(utterance, runs, has_timed_words, &boundaries)
    };
    if spaced.is_empty() {
        return vec![utterance.clone()];
    }
    let mut parts: Vec<Utterance> = Vec::with_capacity(spaced.len() + 1);
    let mut word_lo = 0usize;
    let mut time_lo = utterance.start;
    for k in 0..=spaced.len() {
        let (word_hi, time_hi) = if k < spaced.len() {
            (spaced[k].0 + 1, (spaced[k].1 * 100.0).round() / 100.0)
        } else {
            (words.len(), utterance.end)
        };
        let part_words = slice_part_words(
            utterance,
            &words,
            &word_bounds,
            has_timed_words,
            word_lo,
            word_hi,
        );
        let speaker = dominant_piece_speaker(utterance, runs, time_lo, time_hi);
        let fragment_text = words[word_lo..word_hi].join(" ");
        if let Some(last) = parts.last_mut().filter(|last| last.speaker == speaker) {
            last.end = time_hi;
            if !last.text.is_empty() {
                last.text.push(' ');
            }
            last.text.push_str(&fragment_text);
            last.words.extend(part_words);
        } else {
            parts.push(Utterance {
                start: time_lo,
                end: time_hi,
                speaker,
                channel: utterance.channel,
                text: fragment_text,
                diarized: utterance.diarized,
                silhouette: utterance.silhouette,
                words: part_words,
                ctc_used: utterance.ctc_used,
            });
        }
        word_lo = word_hi;
        time_lo = time_hi;
    }
    if parts.len() <= 1 {
        vec![utterance.clone()]
    } else {
        parts
    }
}

pub(super) fn refine_ch1_utterances(
    utterances: &mut Vec<Utterance>,
    voice_timeline: &[Utterance],
    protected: &HashMap<usize, usize>,
    promoted_utts: &HashMap<usize, usize>,
) {
    let runs = coalesce_clean_runs(voice_timeline);
    if runs.is_empty()
        || !utterances
            .iter()
            .any(|u| u.end - u.start >= REFINE_MIN_UTTERANCE_SEC)
    {
        return;
    }
    let mut refined = Vec::with_capacity(utterances.len());
    for (index, utterance) in utterances.iter().enumerate() {
        // Защищённые и одиночные реплики неизвестных голосов могут иметь иной
        // ярлык в voice_timeline: дробление вернуло бы реплику соседу.
        if utterance.end - utterance.start >= REFINE_MIN_UTTERANCE_SEC
            && !protected.contains_key(&index)
            && !promoted_utts.contains_key(&index)
        {
            refined.extend(split_utterance_by_voice_runs(utterance, &runs));
        } else {
            refined.push(utterance.clone());
        }
    }
    *utterances = refined;
}
