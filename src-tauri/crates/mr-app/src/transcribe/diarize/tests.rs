use std::collections::{HashMap, HashSet};

use crate::transcribe::clean_speech::SpeechSupport;
use crate::transcribe::echo::FrameClass;
use crate::transcribe::{SpeakerInfo, Utterance, WordConf};

use super::constraint_links::cannot_link_units;
use super::core::short_name_allowed;
use super::coverage::restore_protected;
use super::profiles::{eligible_clusters, pick_samples, reassign_short_replies};
use super::reconcile::{
    dedupe_cross_channel, merge_utterances, reconcile_channels, refresh_speaker_stats,
};
use super::segments::{
    constraint_retry_needed, fill_labels_by_time, fill_unit_labels_by_time, group_segments,
    merge_quiet_short_with_evidence, merge_tiny_clusters, promote_unmatched_utterances,
    promoted_utterances, short_join_threshold, smooth_labels_mono, switch_penalty,
};
use super::split::{
    local_region, speaker_change_cuts, split_at_pauses, split_by_word_pauses,
    split_utterance_by_bounds, split_window_region,
};
use super::split::{refine_ch1_utterances, refine_ch1_with_verifier};
use super::text::{move_boundary_words, words_match};
use super::{
    merge_quiet_short, read_profiles_file, split_words_by_bounds, text_similarity, AcoustUnit,
    MIN_LOCAL_EMBED_SEC,
};

fn info(name: &str, ch: i32, emb: Vec<f64>, count: usize, sec: f64) -> SpeakerInfo {
    SpeakerInfo {
        id: format!("spk_{name}"),
        name: name.to_string(),
        channel: Some(ch),
        matched: false,
        matched_name: None,
        matched_pack_id: None,
        matched_pack_name: None,
        similarity: 0.0,
        silhouette: 0.0,
        candidates: vec![],
        embedding: Some(emb),
        samples: vec![],
        utterances_count: count,
        total_speech_sec: sec,
    }
}

pub(super) fn utt(start: f64, end: f64, speaker: &str, ch: i32, text: &str) -> Utterance {
    Utterance {
        start,
        end,
        speaker: speaker.to_string(),
        channel: Some(ch),
        text: text.to_string(),
        diarized: true,
        silhouette: 0.5,
        words: Vec::new(),
        ctc_used: false,
    }
}

pub(super) fn word(text: &str, start: f64, end: f64) -> WordConf {
    WordConf {
        w: text.to_string(),
        conf: 1.0,
        needs_review: false,
        start_sec: start,
        end_sec: end,
    }
}

#[test]
fn boundary_words_move_to_next_utterance() {
    let mut first = utt(0.0, 2.0, "A", 1, "один два три");
    first.words = vec![
        word("один", 0.1, 0.6),
        word("два", 0.7, 1.2),
        word("три", 2.1, 2.5),
    ];
    let mut second = utt(2.0, 4.0, "B", 1, "четыре");
    second.words = vec![word("четыре", 2.6, 3.2)];
    let mut utts = vec![first, second];
    move_boundary_words(&mut utts);
    assert_eq!(utts.len(), 2);
    assert_eq!(utts[0].text, "один два");
    assert_eq!(utts[1].text, "три четыре");
    assert_eq!(utts[1].words.len(), 2);
}

#[test]
fn long_segments_cut_at_vad_minimum_in_second_half() {
    let mut probs = vec![0.9f32; 188];
    probs[31] = 0.0;
    probs[94] = 0.1;
    let out = group_segments(&[(0.0, 6.0)], &probs, 4.0, 0.5, 0);
    assert_eq!(out.len(), 2);
    assert!((out[0].1 - 94.0 * 0.032).abs() < 1e-5);
    assert_eq!(out[1].0, out[0].1);
    assert!(out.iter().all(|(s, e)| e - s <= 4.0));

    let fallback = group_segments(&[(0.0, 6.0)], &[], 4.0, 0.5, 0);
    assert_eq!(fallback, vec![(0.0, 3.0), (3.0, 6.0)]);
}

#[test]
fn merges_same_voice_from_both_channels() {
    let mut infos = vec![
        info("Участник 2", 0, vec![1.0, 0.0, 0.0], 1, 4.0),
        info("Собеседник 1", 1, vec![1.0, 0.0, 0.0], 1, 4.0),
    ];
    let mut utts = vec![
        utt(1.0, 5.0, "Участник 2", 0, "привет как дела"),
        utt(1.2, 5.2, "Собеседник 1", 1, "привет как дела"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].name, "Собеседник 1");
    assert_eq!(utts[0].speaker, "Собеседник 1");
    dedupe_cross_channel(&mut utts, &[], &HashMap::new());
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].channel, Some(1));
}

#[test]
fn merged_channels_centroid_uses_both_embeddings() {
    let a = mr_ml::l2_normalize(&[1.0, 0.0]);
    let b = mr_ml::l2_normalize(&[0.9, 0.1]);
    let mut infos = vec![
        info("Участник 2", 0, a.clone(), 1, 1.0),
        info("Собеседник 1", 1, b.clone(), 1, 9.0),
    ];
    let mut utts = vec![
        utt(1.0, 5.0, "Участник 2", 0, "нейтральная тестовая фраза"),
        utt(1.0, 5.0, "Собеседник 1", 1, "нейтральная тестовая фраза"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 1);
    let embedding = infos[0].embedding.clone().expect("центроид");
    let expected = mr_ml::weighted_centroid(&[a, b], &[1.0, 9.0]);
    let sim = mr_ml::cosine_similarity(&embedding, &expected);
    assert!(sim > 0.999, "слияние потеряло второй вектор: {sim}");
}

#[test]
fn dedupe_cross_channel_drops_echo_even_with_different_speakers() {
    let mut utts = vec![
        utt(1.0, 5.0, "Спикер B", 1, "нейтральная тестовая фраза"),
        utt(1.1, 5.1, "Спикер A", 0, "нейтральная тестовая фраза"),
    ];
    dedupe_cross_channel(&mut utts, &[], &HashMap::new());
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].speaker, "Спикер B");
    assert_eq!(utts[0].channel, Some(1));
}

#[test]
fn dedupe_keeps_different_matched_speakers() {
    let mut a = info("Спикер A", 0, vec![1.0, 0.0], 5, 10.0);
    a.matched = true;
    let mut b = info("Спикер B", 1, vec![0.0, 1.0], 5, 10.0);
    b.matched = true;
    let mut utts = vec![
        utt(1.0, 5.0, "Спикер B", 1, "нейтральная тестовая фраза"),
        utt(1.1, 5.1, "Спикер A", 0, "нейтральная тестовая фраза"),
    ];
    dedupe_cross_channel(&mut utts, &[a, b], &HashMap::new());
    assert_eq!(utts.len(), 2);
}

#[test]
fn dedupe_prefers_louder_channel_as_source() {
    let mut samples = HashMap::new();
    samples.insert(0usize, vec![0.5f32; 16_000 * 6]);
    samples.insert(1usize, vec![0.1f32; 16_000 * 6]);
    let mut utts = vec![
        utt(1.0, 5.0, "Спикер B", 1, "нейтральная тестовая фраза"),
        utt(1.1, 5.1, "Спикер A", 0, "нейтральная тестовая фраза"),
    ];
    dedupe_cross_channel(&mut utts, &[], &samples);
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].speaker, "Спикер A");
    assert_eq!(utts[0].channel, Some(0));
}

#[test]
fn dedupe_keeps_matched_over_louder_unmatched_echo() {
    let mut a = info("Спикер A", 0, vec![1.0, 0.0], 5, 10.0);
    a.matched = true;
    let mut samples = HashMap::new();
    samples.insert(0usize, vec![0.1f32; 16_000 * 6]);
    samples.insert(1usize, vec![0.5f32; 16_000 * 6]);
    let mut utts = vec![
        utt(1.0, 5.0, "Собеседник 1", 1, "нейтральная тестовая фраза"),
        utt(1.1, 5.1, "Спикер A", 0, "нейтральная тестовая фраза"),
    ];
    dedupe_cross_channel(&mut utts, &[a], &samples);
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].speaker, "Спикер A");
    assert_eq!(utts[0].channel, Some(0));
}

#[test]
fn text_similarity_folds_yo_and_detects_containment() {
    let echo = "Всё ещё альфа бета гамма дельта?";
    let original = "Сегодня всё еще альфа бета гамма дельта и дальше.";
    assert!(text_similarity(echo, original) >= 0.85);
}

#[test]
fn words_match_accepts_inflected_four_char_word_forms() {
    assert!(words_match("вода", "вода"));
    assert!(words_match("вода", "воде"));
}

#[test]
fn words_match_rejects_different_lexemes_with_shared_prefix() {
    assert!(!words_match("дыня", "дымка"));
    assert!(!words_match("стол", "стон"));
    assert!(!words_match("work", "word"));
}

#[test]
fn single_word_echo_word_form_counts_as_duplicate() {
    assert!(text_similarity("воде", "вода") >= 0.55);
    assert!(text_similarity("дыня", "дымка") < 0.55);
    assert!(text_similarity("стол", "стон") < 0.55);
    assert!(
        text_similarity("да", "так да уменьшил") < 0.55,
        "backchannel word inside a longer phrase of the other channel is not an echo"
    );
}

#[test]
fn short_backchannel_over_long_phrase_survives_dedupe() {
    let mut utts = vec![
        utt(
            1.0,
            5.0,
            "Спикер B",
            1,
            "длинная нейтральная тестовая фраза",
        ),
        utt(2.0, 2.5, "Спикер A", 0, "да"),
    ];
    dedupe_cross_channel(&mut utts, &[], &HashMap::new());
    assert_eq!(utts.len(), 2);
    assert_eq!(utts[1].speaker, "Спикер A");
}

#[test]
fn full_phrase_echo_still_removed_after_short_reply_guard() {
    let mut utts = vec![
        utt(1.0, 5.0, "Спикер B", 1, "нейтральная тестовая фраза"),
        utt(1.1, 5.1, "Спикер A", 0, "нейтральная тестовая фраза"),
    ];
    dedupe_cross_channel(&mut utts, &[], &HashMap::new());
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].speaker, "Спикер B");
}

#[test]
fn unmatched_clusters_in_one_channel_stay_separate() {
    let mut infos = vec![
        info("Участник 1", 0, vec![1.0, 0.0], 5, 10.0),
        info("Участник 2", 0, vec![1.0, 0.0], 5, 10.0),
    ];
    let mut utts = vec![
        utt(1.0, 3.0, "Участник 1", 0, "первая реплика"),
        utt(10.0, 12.0, "Участник 2", 0, "вторая реплика"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 2);
}

#[test]
fn matched_profile_and_unnamed_tail_in_one_channel_merge() {
    let mut matched = info("Я", 0, vec![1.0, 0.0], 5, 10.0);
    matched.matched = true;
    matched.matched_name = Some("Я".to_string());
    let mut tail = info("Участник 2", 0, vec![1.0, 0.0], 1, 2.0);
    tail.candidates.push(("Я".to_string(), 0.8));
    let mut infos = vec![matched, tail];
    let mut utts = vec![
        utt(1.0, 5.0, "Я", 0, "моя реплика"),
        utt(10.0, 11.0, "Участник 2", 0, "хвост"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].name, "Я");
}

#[test]
fn ineligible_tail_cannot_inherit_a_profile_during_reconciliation() {
    for channel in [0, 1] {
        let mut known = info("A", 0, vec![1.0, 0.0], 5, 10.0);
        known.matched = true;
        let unknown = info("B", channel, vec![1.0, 0.0], 1, 0.8);
        let mut infos = vec![known, unknown];
        let mut utterances = vec![
            utt(0.0, 3.0, "A", 0, "раз два"),
            utt(0.0, 0.8, "B", channel, "раз два"),
        ];
        reconcile_channels(&mut infos, &mut utterances);
        assert_eq!(infos.len(), 2, "канал {channel}");
        assert_eq!(utterances[1].speaker, "B");
    }
}

#[test]
fn unnamed_tail_cannot_inherit_profile_through_another_cluster() {
    let mut known = info("A", 0, vec![1.0, 0.0], 5, 10.0);
    known.matched = true;
    let unknown = info("B", 1, vec![1.0, 0.0], 1, 0.8);
    let mut bridge = info("C", 1, vec![1.0, 0.0], 3, 4.0);
    bridge.candidates.push(("A".to_string(), 0.8));
    let mut infos = vec![known, unknown, bridge];
    let mut utterances = vec![
        utt(0.0, 3.0, "A", 0, "раз два"),
        utt(0.0, 0.8, "B", 1, "раз два"),
        utt(0.0, 3.0, "C", 1, "раз два"),
    ];
    reconcile_channels(&mut infos, &mut utterances);
    assert_ne!(utterances[1].speaker, "A");
}

#[test]
fn similar_voices_across_channels_without_overlap_stay_separate() {
    let mut infos = vec![
        info("Участник 1", 0, vec![1.0, 0.0], 1, 3.0),
        info("Собеседник 1", 1, vec![1.0, 0.0], 1, 3.0),
    ];
    let mut utts = vec![
        utt(1.0, 3.0, "Участник 1", 0, "первая реплика"),
        utt(20.0, 22.0, "Собеседник 1", 1, "вторая реплика"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 2);
    assert_eq!(utts[0].speaker, "Участник 1");
    assert_eq!(utts[1].speaker, "Собеседник 1");
}

#[test]
fn merges_short_echo_even_with_weak_embedding() {
    let mut infos = vec![
        info("Участник 5", 0, vec![1.0, 0.0], 1, 1.8),
        info("Собеседник 1", 1, vec![0.3, 0.95], 1, 3.0),
    ];
    let mut utts = vec![
        utt(10.0, 11.8, "Участник 5", 0, "Слово."),
        utt(9.8, 12.8, "Собеседник 1", 1, "Слово."),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 1);
    assert_eq!(utts[0].speaker, "Собеседник 1");
    assert_eq!(utts[1].speaker, "Собеседник 1");
}

#[test]
fn different_matched_profiles_are_never_merged() {
    let mut a = info("Спикер A", 0, vec![1.0, 0.0], 1, 5.0);
    a.matched = true;
    a.matched_name = Some("Спикер A".to_string());
    let mut b = info("Спикер B", 1, vec![1.0, 0.0], 1, 5.0);
    b.matched = true;
    b.matched_name = Some("Спикер B".to_string());
    let mut infos = vec![a, b];
    let mut utts = vec![
        utt(1.0, 6.0, "Спикер A", 0, "одна и та же фраза"),
        utt(1.0, 6.0, "Спикер B", 1, "одна и та же фраза"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 2);
}

#[test]
fn different_voices_are_not_merged() {
    let mut infos = vec![
        info("Я", 0, vec![1.0, 0.0], 1, 5.0),
        info("Собеседник 1", 1, vec![0.0, 1.0], 1, 5.0),
    ];
    let mut utts = vec![
        utt(1.0, 6.0, "Я", 0, "сегодня хорошая погода"),
        utt(1.0, 6.0, "Собеседник 1", 1, "совсем другой разговор"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 2);
}

#[test]
fn overlapping_same_speaker_keeps_best_copy_not_concat() {
    let utts = vec![
        utt(10.0, 20.0, "Собеседник 1", 0, "один два три четыре"),
        utt(10.2, 20.2, "Собеседник 1", 1, "один два три четыре пять"),
    ];
    let merged = merge_utterances(utts);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].text, "один два три четыре пять");
}

#[test]
fn same_channel_padding_overlap_keeps_both_texts() {
    let utts = vec![
        utt(10.0, 20.0, "Я", 0, "первая часть фразы"),
        utt(19.5, 21.0, "Я", 0, "вторая часть фразы"),
    ];
    let merged = merge_utterances(utts);
    assert_eq!(merged.len(), 1);
    assert!(merged[0].text.contains("вторая"));
    assert!(merged[0].text.contains("первая"));

    let utts = vec![
        utt(10.0, 20.0, "Я", 0, "начало длинной тестовой фразы"),
        utt(
            19.5,
            21.0,
            "Я",
            0,
            "начало длинной тестовой фразы продолжение",
        ),
    ];
    let merged = merge_utterances(utts);
    assert_eq!(merged[0].text, "начало длинной тестовой фразы продолжение");
}

#[test]
fn same_channel_repeated_words_are_not_dropped() {
    let utts = vec![
        utt(1.0, 2.0, "Я", 0, "ну в общем"),
        utt(2.1, 3.0, "Я", 0, "в общем"),
    ];
    let merged = merge_utterances(utts);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].text, "ну в общем в общем");
}

#[test]
fn non_overlapping_same_speaker_is_joined() {
    let utts = vec![
        utt(1.0, 2.0, "Я", 0, "первая фраза"),
        utt(2.4, 3.0, "Я", 0, "вторая фраза"),
    ];
    let merged = merge_utterances(utts);
    assert_eq!(merged.len(), 1);
    assert!(merged[0].text.contains("первая"));
    assert!(merged[0].text.contains("вторая"));
}

#[test]
fn speaker_stats_follow_final_utterances() {
    let mut infos = vec![
        info("Я", 0, vec![1.0, 0.0], 9, 99.0),
        info("Пропавший", 0, vec![0.0, 1.0], 9, 99.0),
    ];
    let utts = vec![utt(1.0, 3.0, "Я", 0, "раз"), utt(3.5, 4.0, "Я", 0, "два")];
    refresh_speaker_stats(&mut infos, &utts);
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].utterances_count, 2);
    assert_eq!(infos[0].total_speech_sec, 2.5);
}

#[test]
fn short_word_is_not_lost_as_substring() {
    let utts = vec![utt(1.0, 2.0, "Я", 0, "да"), utt(2.2, 3.0, "Я", 0, "давай")];
    let merged = merge_utterances(utts);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].text, "да давай");
}

#[test]
fn merge_quiet_short_keeps_loud_separate() {
    let mut samples = vec![0.005f32; 16 * 16000];
    for x in samples[6 * 16000..7 * 16000].iter_mut() {
        *x = 0.1;
    }
    let groups = vec![
        (0.0, 1.0),
        (2.0, 3.0),
        (4.0, 5.0),
        (6.0, 7.0),
        (8.0, 9.0),
        (10.0, 13.0),
    ];
    let out = merge_quiet_short(&samples, groups, 0);
    assert_eq!(out, vec![(0.0, 5.0), (6.0, 7.0), (8.0, 9.0), (10.0, 13.0)]);
}

#[test]
fn merge_quiet_short_caps_merged_duration() {
    let samples = vec![0.005f32; 16 * 16000];
    let groups: Vec<(f32, f32)> = (0..8)
        .map(|i| (i as f32 * 2.0, i as f32 * 2.0 + 1.0))
        .collect();
    let out = merge_quiet_short(&samples, groups, 0);
    assert_eq!(out.len(), 2);
    assert!(out[0].1 - out[0].0 <= 12.0);
}

fn echo_evidence(
    total_sec: f64,
    spans: &[(f64, f64, crate::transcribe::echo::FrameClass)],
) -> crate::transcribe::echo::EchoEvidence {
    let win = mr_ml::onnx::vad::VAD_WINDOW as f64 / 16000.0;
    let n = (total_sec / win).ceil() as usize;
    let frames = (0..n)
        .map(|i| {
            let t = i as f64 * win;
            spans
                .iter()
                .find(|(a, b, _)| t >= *a && t < *b)
                .map(|(_, _, class)| *class)
                .unwrap_or(crate::transcribe::echo::FrameClass::Uncertain)
        })
        .collect::<Vec<_>>();
    crate::transcribe::echo::EchoEvidence {
        delay_samples: 1000,
        delay_sec: 0.0625,
        prominence: 12.0,
        stability: 1.0,
        gain: 0.1,
        noise_floor: 1e-4,
        residual: vec![0.0; n],
        frames,
    }
}

#[test]
fn merge_quiet_short_keeps_local_speech_apart_from_echo() {
    use crate::transcribe::echo::FrameClass;
    let samples = vec![0.005f32; 6 * 16000];
    let groups = vec![(0.0, 1.0), (2.0, 3.0)];
    assert_eq!(
        merge_quiet_short(&samples, groups.clone(), 0),
        vec![(0.0, 3.0)]
    );

    let ev = echo_evidence(
        6.0,
        &[
            (0.0, 1.0, FrameClass::LocalOnly),
            (1.0, 2.0, FrameClass::Uncertain),
            (2.0, 3.0, FrameClass::EchoOnly),
        ],
    );
    let out = merge_quiet_short_with_evidence(&samples, groups, 0, Some(&ev), None);
    assert_eq!(out, vec![(0.0, 1.0), (2.0, 3.0)]);
}

#[test]
fn merge_quiet_short_joins_local_segments_with_evidence() {
    use crate::transcribe::echo::FrameClass;
    let samples = vec![0.005f32; 6 * 16000];
    let groups = vec![(0.0, 1.0), (2.0, 3.0)];
    let ev = echo_evidence(
        6.0,
        &[
            (0.0, 1.0, FrameClass::LocalOnly),
            (1.0, 2.0, FrameClass::Uncertain),
            (2.0, 3.0, FrameClass::DoubleTalk),
        ],
    );
    let out = merge_quiet_short_with_evidence(&samples, groups, 0, Some(&ev), None);
    assert_eq!(out, vec![(0.0, 3.0)]);
}

#[test]
fn merge_quiet_short_does_not_bridge_echo_gap() {
    use crate::transcribe::echo::FrameClass;
    let samples = vec![0.005f32; 6 * 16000];
    let groups = vec![(0.0, 1.0), (3.0, 4.0)];
    let ev = echo_evidence(
        6.0,
        &[
            (0.0, 1.0, FrameClass::LocalOnly),
            (1.0, 3.0, FrameClass::EchoOnly),
            (3.0, 4.0, FrameClass::LocalOnly),
        ],
    );
    let out = merge_quiet_short_with_evidence(&samples, groups, 0, Some(&ev), None);
    assert_eq!(out, vec![(0.0, 1.0), (3.0, 4.0)]);
}

#[test]
fn merge_quiet_short_does_not_join_echo_tail_with_system_active() {
    use crate::transcribe::echo::FrameClass;
    let samples = vec![0.005f32; 6 * 16000];
    let system = vec![0.05f32; 6 * 16000];
    let groups = vec![(0.0, 1.0), (2.0, 3.0)];
    let ev = echo_evidence(
        6.0,
        &[
            (0.0, 1.0, FrameClass::LocalOnly),
            (1.0, 2.0, FrameClass::Uncertain),
            (2.0, 3.0, FrameClass::Uncertain),
        ],
    );
    let out = merge_quiet_short_with_evidence(&samples, groups, 0, Some(&ev), Some(&system));
    assert_eq!(out, vec![(0.0, 1.0), (2.0, 3.0)]);
}

#[test]
fn split_words_no_panic_on_late_bounds() {
    let words: Vec<&str> = (0..12).map(|_| "w").collect();
    let bounds = vec![0.0, 1.0, 2.0, 3.0, 20.0];
    let texts = split_words_by_bounds(&words, &bounds);
    assert_eq!(texts.len(), 4);
    assert_eq!(
        texts
            .iter()
            .map(|t| t.split_whitespace().count())
            .sum::<usize>(),
        words.len()
    );
}

#[test]
fn split_words_all_words_distributed() {
    let words = vec!["a", "b", "c", "d", "e"];
    let texts = split_words_by_bounds(&words, &[0.0, 2.0, 4.0]);
    assert_eq!(texts.len(), 2);
    assert_eq!(texts.join(" "), "a b c d e");
}

#[test]
fn split_words_last_fragment_gets_word_when_words_remain() {
    let words = vec!["a", "b", "c", "d"];
    let texts = split_words_by_bounds(&words, &[0.0, 1.0, 4.0, 5.0]);
    assert_eq!(texts.len(), 3);
    assert!(texts.iter().all(|t| !t.is_empty()), "{texts:?}");
    assert_eq!(texts.join(" "), "a b c d");
}

#[test]
fn short_join_threshold_follows_embedding_calibration() {
    assert!((short_join_threshold(0.4) - 0.36).abs() < 1e-9);
    assert!((short_join_threshold(1.0) - 0.45).abs() < 1e-9);
    assert!((short_join_threshold(1.5) - 0.48).abs() < 1e-9);
    assert!(short_join_threshold(0.6) < 0.65);
}

#[test]
fn profile_loader_skips_foreign_embedding_space_and_uses_prototypes() {
    let dir = std::env::temp_dir().join(format!("mr-vp-space-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("voiceprints.json");
    let emb = vec![0.1f64; mr_ml::EMBEDDING_DIM];
    std::fs::write(
        &file,
        serde_json::to_string(&serde_json::json!({
            "version": 3,
            "embedding_space": "another-space",
            "profiles": [{"name": "Чужой", "embedding": emb.clone()}],
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(read_profiles_file(&file).is_empty());

    std::fs::write(
        &file,
        serde_json::to_string(&serde_json::json!({
            "version": 3,
            "embedding_space": mr_ml::EMBEDDING_SPACE,
            "profiles": [{
                "name": "Свой",
                "prototypes": [{
                    "id": "p1", "embedding": emb, "duration_sec": 4.0,
                    "quality": 1.0, "created_at": "", "snippet_relpath": null
                }]
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    let got = read_profiles_file(&file);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].embedding.len(), mr_ml::EMBEDDING_DIM);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn profile_loader_recomputes_drifted_centroid_from_prototypes() {
    let dir = std::env::temp_dir().join(format!("mr-vp-drift-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("voiceprints.json");
    let mut a = vec![0.0f64; mr_ml::EMBEDDING_DIM];
    a[0] = 1.0;
    let mut b = vec![0.0f64; mr_ml::EMBEDDING_DIM];
    b[1] = 1.0;
    std::fs::write(
            &file,
            serde_json::to_string(&serde_json::json!({
                "version": 3,
                "embedding_space": mr_ml::EMBEDDING_SPACE,
                "profiles": [{
                    "name": "Свой",
                    "embedding": a.clone(),
                    "samples_count": 2,
                    "prototypes": [
                        {"id": "p1", "embedding": a, "duration_sec": 4.0, "quality": 1.0, "created_at": ""},
                        {"id": "p2", "embedding": b, "duration_sec": 4.0, "quality": 1.0, "created_at": ""},
                    ]
                }],
            }))
            .unwrap(),
        )
        .unwrap();
    let got = read_profiles_file(&file);
    assert_eq!(got.len(), 1);
    let expected = mr_ml::weighted_centroid(&[vec![1.0, 0.0], vec![0.0, 1.0]], &[4.0, 4.0]);
    let flat: Vec<f64> = got[0].embedding[..2].to_vec();
    assert!(mr_ml::cosine_similarity(&flat, &expected) > 0.999);
    assert!((flat[0] - flat[1]).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pauses_split_long_utterance_without_losing_words() {
    let sr = 16000usize;
    let mut samples = vec![0.1f32; 12 * sr];
    for x in samples[5 * sr..5 * sr + sr / 3].iter_mut() {
        *x = 0.0;
    }
    for x in samples[9 * sr..9 * sr + sr / 3].iter_mut() {
        *x = 0.0;
    }
    let utt = || Utterance {
        start: 0.0,
        end: 12.0,
        speaker: "X".to_string(),
        channel: None,
        text: "один два три четыре пять шесть семь восемь".to_string(),
        diarized: false,
        silhouette: 0.0,
        words: Vec::new(),
        ctc_used: false,
    };
    let out = split_at_pauses(&samples, &[utt()]);
    assert!(out.len() >= 3);
    assert!(out.iter().all(|u| u.end - u.start <= 6.01));
    assert_eq!(
        out.iter()
            .map(|u| u.text.clone())
            .collect::<Vec<_>>()
            .join(" "),
        "один два три четыре пять шесть семь восемь"
    );
    // Тихая копия режется там же: порог паузы не зависит от громкости.
    let quiet: Vec<f32> = samples.iter().map(|x| x * 0.01).collect();
    let out_quiet = split_at_pauses(&quiet, &[utt()]);
    assert_eq!(out.len(), out_quiet.len());
}

#[test]
fn pick_samples_prefers_well_separated_voice() {
    let mut good = utt(10.0, 14.0, "A", 0, "чистая обособленная реплика");
    good.silhouette = 0.9;
    let mut noisy = utt(1.0, 5.0, "A", 0, "чистая обособленная реплика");
    noisy.silhouette = -0.5;
    let samples = pick_samples(&[&noisy, &good], 1);
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].start, 10.0);
}

#[test]
fn sample_window_stays_inside_utterance() {
    let utterance = utt(10.0, 13.96, "A", 0, "чистая обособленная реплика");
    let samples = pick_samples(&[&utterance], 1);
    assert!(samples[0].start >= 10.0, "{samples:?}");
    assert!(
        samples[0].end <= 13.96,
        "окно сэмпла вышло за реплику: {samples:?}"
    );
}

#[test]
fn fill_unit_labels_by_time_uses_nearest_unit_inside_utterance() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 1.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 0,
            start: 5.0,
            end: 6.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
    ];
    let mut labels = std::collections::HashMap::from([(0usize, 7usize)]);
    fill_unit_labels_by_time(&units, &mut labels);
    assert_eq!(labels.get(&1), Some(&7));
}

#[test]
fn fill_unit_labels_by_time_does_not_cross_utterance_boundary() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 1.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 5.0,
            end: 6.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
    ];
    let mut labels = std::collections::HashMap::from([(0usize, 7usize)]);
    fill_unit_labels_by_time(&units, &mut labels);
    assert!(!labels.contains_key(&1));
}

#[test]
fn unmatched_utterance_becomes_its_own_speaker() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 2.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 10.0,
            end: 10.9,
            dur: 0.9,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
        AcoustUnit {
            utterance: 2,
            start: 20.0,
            end: 20.5,
            dur: 0.5,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
    ];
    let mut labels = std::collections::HashMap::from([(0usize, 0usize)]);
    let mut next = 1usize;
    let promoted = promote_unmatched_utterances(
        &units,
        &mut labels,
        &mut next,
        0,
        SpeechSupport::default(),
        None,
    );
    assert_eq!(promoted, vec![1]);
    assert_eq!(labels.get(&1), Some(&1));
    assert!(!labels.contains_key(&2));
    assert_eq!(next, 2);
}

#[test]
fn unmatched_utterance_touching_previous_is_its_continuation() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 2.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 2.0,
            end: 3.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
    ];
    let mut labels = std::collections::HashMap::from([(0usize, 0usize)]);
    let mut next = 1usize;
    let promoted = promote_unmatched_utterances(
        &units,
        &mut labels,
        &mut next,
        0,
        SpeechSupport::default(),
        None,
    );
    assert!(promoted.is_empty());
    assert!(!labels.contains_key(&1));
}

#[test]
fn unmatched_utterances_of_one_voice_share_a_label() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 2.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 10.0,
            end: 11.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
        AcoustUnit {
            utterance: 2,
            start: 20.0,
            end: 21.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![0.01, 1.0],
        },
    ];
    let mut labels = std::collections::HashMap::from([(0usize, 0usize)]);
    let mut next = 1usize;
    let promoted = promote_unmatched_utterances(
        &units,
        &mut labels,
        &mut next,
        0,
        SpeechSupport::default(),
        None,
    );
    assert_eq!(promoted, vec![1]);
    assert_eq!(labels.get(&1), Some(&1));
    assert_eq!(labels.get(&2), Some(&1));
}

#[test]
fn unmatched_utterance_respects_speaker_limit() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 2.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 10.0,
            end: 11.0,
            dur: 1.0,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
    ];
    let mut labels = std::collections::HashMap::from([(0usize, 0usize)]);
    let mut next = 1usize;
    let promoted = promote_unmatched_utterances(
        &units,
        &mut labels,
        &mut next,
        1,
        SpeechSupport::default(),
        None,
    );
    assert!(promoted.is_empty());
    assert!(!labels.contains_key(&1));
}

#[test]
fn overlapping_units_do_not_inflate_promotion_duration() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 2.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 10.0,
            end: 10.45,
            dur: 0.45,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 10.0,
            end: 10.45,
            dur: 0.45,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
    ];
    let mut labels = HashMap::from([(0usize, 0usize)]);
    let mut next = 1usize;
    let promoted = promote_unmatched_utterances(
        &units,
        &mut labels,
        &mut next,
        0,
        SpeechSupport::default(),
        None,
    );
    assert!(promoted.is_empty());
    assert!(!labels.contains_key(&1));
    assert!(!labels.contains_key(&2));
}

#[test]
fn promoted_utterances_survive_coverage_renumbering() {
    let mut utterances = vec![
        utt(0.0, 8.0, "", 0, "one two three four"),
        utt(10.0, 12.0, "", 0, "five six"),
        utt(20.0, 22.0, "", 0, "seven eight"),
    ];
    let mut units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 6.0,
            dur: 6.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 0,
            start: 6.0,
            end: 8.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 10.0,
            end: 12.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![0.0, -1.0],
        },
        AcoustUnit {
            utterance: 2,
            start: 20.0,
            end: 22.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![0.0, -1.0],
        },
    ];
    let mut unit_labels = HashMap::from([(0usize, 0usize), (1, 1), (2, 3), (3, 3)]);
    super::coverage::split_covered_utterances(
        &mut utterances,
        &mut units,
        &mut unit_labels,
        SpeechSupport::default(),
    );
    assert_eq!(utterances.len(), 4, "первая реплика должна разделиться");
    let promoted_units: Vec<usize> = units
        .iter()
        .enumerate()
        .filter(|(index, _)| unit_labels.get(index) == Some(&3))
        .map(|(_, unit)| unit.utterance)
        .collect();
    assert_eq!(
        promoted_units,
        vec![2, 3],
        "после split метки неизвестных голосов должны переехать на новые индексы"
    );
    // После split: 0–1 — части первой реплики, 2–3 — неизвестные голоса.
    let label_of = HashMap::from([(0usize, 0usize), (1, 1), (2, 3), (3, 3)]);
    let promoted = promoted_utterances(&label_of, &HashSet::from([3usize]));
    assert_eq!(
        promoted,
        HashMap::from([(2usize, 3usize), (3usize, 3usize)])
    );
    assert!(
        !promoted.contains_key(&1),
        "часть чужой реплики не должна получить метку неизвестного"
    );
}

#[test]
fn cannot_link_units_separates_voices_inside_one_utterance() {
    let frame = mr_ml::onnx::segmentation::FRAME_SAMPLES;
    let mut classes = vec![1u8; 100];
    classes.extend(vec![2u8; 100]);
    let segmentation = mr_ml::onnx::segmentation::Segmentation {
        frames: vec![],
        windows: vec![mr_ml::onnx::segmentation::SegmentationWindow {
            start_sample: 0,
            classes,
        }],
    };
    let frame_sec = frame as f64 / 16000.0;
    let unit = |utterance: usize, from: f64, to: f64| AcoustUnit {
        utterance,
        start: from,
        end: to,
        dur: to - from,
        quality: 1.0,
        embedding: vec![],
    };
    let units = vec![
        unit(0, 0.0, 100.0 * frame_sec),
        unit(0, 100.0 * frame_sec, 200.0 * frame_sec),
    ];
    let core_pos = HashMap::from([(0usize, 0usize), (1, 1)]);
    assert_eq!(
        cannot_link_units(
            &units,
            &core_pos,
            SpeechSupport {
                segmentation: Some(&segmentation),
                vad_probs: None,
            },
            &[],
        ),
        vec![(0, 1)]
    );
    assert!(cannot_link_units(&units, &core_pos, SpeechSupport::default(), &[]).is_empty());
}

#[test]
fn constraint_retry_needed_only_after_unconstrained_collapse_to_one() {
    assert!(constraint_retry_needed(false, false, 1, true));
    assert!(!constraint_retry_needed(false, false, 2, true));
    assert!(!constraint_retry_needed(false, false, 1, false));
    assert!(!constraint_retry_needed(false, true, 1, true));
    assert!(!constraint_retry_needed(true, false, 1, true));
}

#[test]
fn speaker_change_cuts_snaps_cut_to_quiet_frame_between_windows() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let wins: Vec<(f64, Vec<f64>)> = [0.0, 0.5, 1.0, 1.5, 2.0]
        .into_iter()
        .map(|t| (t, if t < 2.0 { a.clone() } else { b.clone() }))
        .collect();
    let mut samples = vec![0.2f32; 16000 * 5];
    for x in samples[16000 * 19 / 10..16000 * 21 / 10].iter_mut() {
        *x = 0.0;
    }
    let cuts = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &[]);
    assert_eq!(cuts.len(), 1);
    assert!((1.9..=2.1).contains(&cuts[0]), "{cuts:?}");
}

#[test]
fn speaker_change_cuts_snaps_to_inter_word_pause() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let wins: Vec<(f64, Vec<f64>)> = vec![
        (0.0, a.clone()),
        (1.0, a.clone()),
        (2.0, a),
        (3.0, b.clone()),
        (4.0, b),
    ];
    let words = vec![
        WordConf {
            w: "первый".to_string(),
            conf: 1.0,
            needs_review: false,
            start_sec: 0.5,
            end_sec: 1.9,
        },
        WordConf {
            w: "второй".to_string(),
            conf: 1.0,
            needs_review: false,
            start_sec: 3.2,
            end_sec: 3.3,
        },
        WordConf {
            w: "третий".to_string(),
            conf: 1.0,
            needs_review: false,
            start_sec: 3.4,
            end_sec: 4.5,
        },
    ];
    let samples = vec![0.0f32; 16000 * 5];
    let cuts = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &words);
    assert_eq!(cuts.len(), 1);
    // Длинная пауза 1.3 с дальше от середины 3.3, но перевешивает короткую 0.1 с.
    assert_eq!(cuts[0], 2.55);
}

#[test]
fn speaker_change_cuts_prefers_quiet_gap_over_loud_long_pause() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let wins: Vec<(f64, Vec<f64>)> = vec![
        (0.0, a.clone()),
        (1.0, a.clone()),
        (2.0, a),
        (3.0, b.clone()),
        (4.0, b),
    ];
    let word = |start_sec: f64, end_sec: f64| WordConf {
        w: "слово".to_string(),
        conf: 1.0,
        needs_review: false,
        start_sec,
        end_sec,
    };
    let words = vec![word(0.5, 1.9), word(3.2, 3.3), word(3.4, 4.5)];
    let mut samples = vec![0.0f32; 16000 * 5];
    for x in samples[16000 * 2525 / 1000..16000 * 2575 / 1000].iter_mut() {
        *x = 0.5;
    }
    let cuts = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &words);
    assert_eq!(cuts.len(), 1);
    // Тишина важнее длительности: короткая тихая пауза у 3.35 выигрывает у громкой длинной 2.55.
    assert_eq!(cuts[0], 3.35);
}

#[test]
fn switch_penalty_drops_with_pause() {
    assert!(switch_penalty(0.5) < switch_penalty(0.0));
    assert!((switch_penalty(0.0) - 0.18).abs() < 1e-12);
    assert!(switch_penalty(2.0) < 0.05);
}

#[test]
fn temporal_smoothing_removes_single_utterance_flip() {
    let utts: Vec<Utterance> = (0..5)
        .map(|i| utt(i as f64, i as f64 + 1.0, "Я", 0, "x"))
        .collect();
    let a = vec![1.0, 0.0];
    let noisy = vec![0.9, 0.1];
    let embs = vec![
        Some(a.clone()),
        Some(a.clone()),
        Some(noisy),
        Some(a.clone()),
        Some(a.clone()),
    ];
    let durs = vec![1.0; 5];
    let quals = vec![1.0; 5];
    let mut labels: std::collections::HashMap<usize, usize> =
        [(0, 10), (1, 10), (2, 11), (3, 10), (4, 10)]
            .into_iter()
            .collect();
    smooth_labels_mono(&utts, &embs, &durs, &quals, &mut labels);
    assert!(labels.values().all(|&l| l == 10));

    let b = vec![0.0, 1.0];
    let embs = vec![
        Some(a.clone()),
        Some(a),
        Some(b.clone()),
        Some(b.clone()),
        Some(b),
    ];
    let mut labels: std::collections::HashMap<usize, usize> =
        [(0, 0), (1, 0), (2, 1), (3, 1), (4, 1)]
            .into_iter()
            .collect();
    smooth_labels_mono(&utts, &embs, &durs, &quals, &mut labels);
    assert_eq!(labels[&0], labels[&1]);
    assert_eq!(labels[&2], labels[&4]);
    assert_ne!(labels[&0], labels[&2]);
}

#[test]
fn tiny_noise_cluster_joins_nearest_voice_but_foreign_stays() {
    let vecs = vec![
        vec![1.0, 0.0],
        vec![1.0, 0.0],
        vec![0.0, 1.0],
        vec![0.0, 1.0],
        vec![0.7, 0.7],
    ];
    let weights = vec![10.0, 10.0, 10.0, 10.0, 1.0];
    let mut labels = vec![0usize, 0, 1, 1, 2];
    merge_tiny_clusters(&vecs, &mut labels, &weights, 1, &[], &[]);
    assert!(labels[4] == 0 || labels[4] == 1);
    let mut distinct = vec![0usize, 0, 1, 1, 2];
    let mut vecs2 = vecs.clone();
    vecs2[4] = vec![-1.0, 0.0];
    merge_tiny_clusters(&vecs2, &mut distinct, &weights, 1, &[], &[]);
    assert_eq!(distinct[4], 2);
}

#[test]
fn tiny_cluster_preserves_cannot_link_with_similar_dominant_voice() {
    let vecs = vec![vec![1.0, 0.0], vec![1.0, 0.0], vec![0.9, 0.1]];
    let mut labels = vec![0, 0, 1];
    merge_tiny_clusters(&vecs, &mut labels, &[10.0, 10.0, 1.0], 1, &[], &[(1, 2)]);
    assert_ne!(
        labels[1], labels[2],
        "cannot-link must survive tiny-cluster merging"
    );
}

#[test]
fn tiny_cluster_matching_known_profile_is_protected() {
    let vecs = vec![
        vec![1.0, 0.0],
        vec![1.0, 0.0],
        vec![0.0, 1.0],
        vec![0.0, 1.0],
        vec![0.7, 0.7],
    ];
    let weights = vec![10.0, 10.0, 10.0, 10.0, 1.0];
    let mut labels = vec![0usize, 0, 1, 1, 2];
    let profile = mr_ml::SpeakerProfile {
        name: "Известный Спикер".to_string(),
        embedding: vec![0.7, 0.7],
        pack_id: String::new(),
        pack_name: String::new(),
        samples_count: 1,
        updated_at: String::new(),
        prototypes: vec![],
    };
    merge_tiny_clusters(&vecs, &mut labels, &weights, 1, &[profile], &[]);
    assert_eq!(labels[4], 2);
}

#[test]
fn short_but_distinct_voice_is_not_merged_into_dominant() {
    let vecs = vec![
        vec![1.0, 0.0],
        vec![1.0, 0.0],
        vec![1.0, 0.0],
        vec![0.0, 1.0],
        vec![0.0, 1.0],
    ];
    let weights = vec![40.0, 40.0, 40.0, 0.5, 0.5];
    let mut labels = vec![0usize, 0, 0, 1, 1];
    merge_tiny_clusters(&vecs, &mut labels, &weights, 1, &[], &[]);
    assert_eq!(labels[3], 1);
    assert_eq!(labels[4], 1);
}

#[test]
fn unlabelled_short_gets_nearest_time_label_without_panic() {
    let utts = vec![
        utt(0.0, 2.0, "A", 0, "a"),
        utt(10.0, 12.0, "B", 0, "b"),
        utt(9.0, 9.5, "A", 0, "short"),
    ];
    let mut labels = std::collections::HashMap::from([(0, 7), (1, 8)]);
    fill_labels_by_time(&utts, &mut labels);
    assert_eq!(labels[&2], 8);
}

#[test]
fn split_words_few_words_no_panic() {
    let words = vec!["one"];
    let texts = split_words_by_bounds(&words, &[0.0, 1.0, 2.0, 3.0]);
    assert_eq!(texts.len(), 3);
    assert_eq!(texts[0], "one");
    assert_eq!(texts[1], "");
    assert_eq!(texts[2], "");
}

#[test]
fn split_words_snaps_to_punctuation() {
    // Пропорция со сдвигом должна аккуратно привязываться к точке,
    // не отрывая первое слово следующего предложения.
    let words = vec![
        "Первая",
        "фраза",
        "завершена.",
        "Вторая",
        "фраза",
        "началась",
    ];
    let bounds = vec![0.0, 2.8, 6.0];
    let texts = split_words_by_bounds(&words, &bounds);
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], "Первая фраза завершена.");
    assert_eq!(texts[1], "Вторая фраза началась");
}

#[test]
fn pauses_isolate_short_dialog_response() {
    let sr = 16000usize;
    let mut samples = vec![0.1f32; 6 * sr];
    for x in samples[3 * sr..(3 * sr + (sr * 22 / 100))].iter_mut() {
        *x = 0.0;
    }
    for x in samples[37 * sr / 10..(37 * sr / 10 + (sr * 22 / 100))].iter_mut() {
        *x = 0.0;
    }
    let u = Utterance {
        start: 0.0,
        end: 6.0,
        speaker: "X".to_string(),
        channel: None,
        text: "Вводная фраза первого спикера. Короткий ответ. Финальная фраза разговора."
            .to_string(),
        diarized: false,
        silhouette: 0.0,
        words: Vec::new(),
        ctc_used: false,
    };
    let out = split_at_pauses(&samples, &[u]);
    assert_eq!(out.len(), 3);
    assert!(out[1].text.contains("Короткий ответ"));
    assert_eq!(
        out.iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
            .join(" "),
        "Вводная фраза первого спикера. Короткий ответ. Финальная фраза разговора."
    );
}

#[test]
fn continuous_speech_with_micro_closures_is_not_fragmented() {
    let sr = 16000usize;
    let mut samples = vec![0.1f32; (4.5 * sr as f64) as usize];
    // Микросмычки согласных 40 мс (2 кадра) на 1.0 с и 2.5 с — не должны резать
    for x in samples[sr..sr + (sr * 4 / 100)].iter_mut() {
        *x = 0.0;
    }
    for x in samples[25 * sr / 10..25 * sr / 10 + (sr * 4 / 100)].iter_mut() {
        *x = 0.0;
    }
    let u = Utterance {
        start: 0.0,
        end: 4.5,
        speaker: "X".to_string(),
        channel: None,
        text: "Непрерывный поток слов без настоящих пауз в диалоге".to_string(),
        diarized: false,
        silhouette: 0.0,
        words: Vec::new(),
        ctc_used: false,
    };
    let out = split_at_pauses(&samples, &[u]);
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0].text,
        "Непрерывный поток слов без настоящих пауз в диалоге"
    );
}

#[test]
fn pause_discrimination_distinguishes_closures_from_dialog_turns() {
    let sr = 16000usize;
    // 1. Пауза 90 мс (смычка согласных / микрозаминка) — НЕ должна резать
    let mut samples_closure = vec![0.1f32; 4 * sr];
    for x in samples_closure[2 * sr..(2 * sr + (sr * 9 / 100))].iter_mut() {
        *x = 0.0;
    }
    let u1 = Utterance {
        start: 0.0,
        end: 4.0,
        speaker: "X".to_string(),
        channel: None,
        text: "Фраза со смычкой внутри слова".to_string(),
        diarized: false,
        silhouette: 0.0,
        words: Vec::new(),
        ctc_used: false,
    };
    let out1 = split_at_pauses(&samples_closure, &[u1]);
    assert_eq!(out1.len(), 1, "90 мс пауза не должна дробить реплику");

    let mut samples_turn = vec![0.1f32; 4 * sr];
    for x in samples_turn[2 * sr..(2 * sr + (sr * 15 / 100))].iter_mut() {
        *x = 0.0;
    }
    let u2 = Utterance {
        start: 0.0,
        end: 4.0,
        speaker: "X".to_string(),
        channel: None,
        text: "Первая реплика диалога. Ответная реплика собеседника.".to_string(),
        diarized: false,
        silhouette: 0.0,
        words: Vec::new(),
        ctc_used: false,
    };
    let out2 = split_at_pauses(&samples_turn, &[u2]);
    assert_eq!(
        out2.len(),
        2,
        "150 мс пауза должна разделять реплики в диалоге"
    );
}

#[test]
fn pauses_split_long_window_leaving_no_chunks_over_max_duration() {
    let sr = 16000usize;
    let total_sec = 20.0f64;
    let mut samples = vec![0.1f32; (total_sec * sr as f64) as usize];
    for sec in 1..20 {
        let start = sec * sr;
        let end = start + (sr * 22 / 100);
        for x in samples[start..end].iter_mut() {
            *x = 0.0;
        }
    }
    let words: Vec<String> = (1..=40).map(|i| format!("слово{i}")).collect();
    let u = Utterance {
        start: 0.0,
        end: total_sec,
        speaker: "X".to_string(),
        channel: None,
        text: words.join(" "),
        diarized: false,
        silhouette: 0.0,
        words: Vec::new(),
        ctc_used: false,
    };
    let out = split_at_pauses(&samples, &[u]);
    assert!(
        out.len() >= 15,
        "длинное окно должно быть нарезано по регулярным паузам: {}",
        out.len()
    );
    for seg in &out {
        let dur = seg.end - seg.start;
        assert!(
            dur <= 6.6,
            "сегмент {}-{} слишком длинный ({dur} с > 6.6 с)",
            seg.start,
            seg.end
        );
    }
    assert_eq!(out.first().unwrap().start, 0.0);
    assert_eq!(out.last().unwrap().end, total_sec);
}

#[test]
fn split_utterance_by_bounds_preserves_ctc_word_timestamps() {
    let u = Utterance {
        start: 0.0,
        end: 5.0,
        speaker: "X".to_string(),
        channel: None,
        text: "слово1 слово2 слово3 слово4".to_string(),
        diarized: false,
        silhouette: 0.0,
        words: vec![
            crate::transcribe::WordConf {
                w: "слово1".to_string(),
                conf: 0.99,
                needs_review: false,
                start_sec: 0.2,
                end_sec: 0.6,
            },
            crate::transcribe::WordConf {
                w: "слово2".to_string(),
                conf: 0.98,
                needs_review: false,
                start_sec: 0.8,
                end_sec: 1.2,
            },
            crate::transcribe::WordConf {
                w: "слово3".to_string(),
                conf: 0.97,
                needs_review: false,
                start_sec: 3.2,
                end_sec: 3.6,
            },
            crate::transcribe::WordConf {
                w: "слово4".to_string(),
                conf: 0.96,
                needs_review: false,
                start_sec: 3.8,
                end_sec: 4.2,
            },
        ],
        ctc_used: true,
    };
    let bounds = vec![0.0, 2.0, 5.0];
    let parts = split_utterance_by_bounds(&u, &bounds);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].text, "слово1 слово2");
    assert_eq!(parts[0].words.len(), 2);
    assert_eq!(parts[0].words[0].w, "слово1");
    assert_eq!(parts[0].words[1].w, "слово2");
    assert_eq!(parts[1].text, "слово3 слово4");
    assert_eq!(parts[1].words.len(), 2);
    assert_eq!(parts[1].words[0].w, "слово3");
    assert_eq!(parts[1].words[1].w, "слово4");
}

#[test]
fn reconcile_channels_blocks_candidate_conflict() {
    let mut infos = vec![
        SpeakerInfo {
            id: "spk_1".to_string(),
            name: "Спикер A".to_string(),
            channel: Some(0),
            matched: true,
            matched_name: Some("Спикер A".to_string()),
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: 0.70,
            silhouette: 0.5,
            candidates: vec![("Спикер A".to_string(), 0.70)],
            embedding: Some(vec![1.0; mr_ml::EMBEDDING_DIM]),
            samples: vec![],
            utterances_count: 5,
            total_speech_sec: 10.0,
        },
        SpeakerInfo {
            id: "spk_2".to_string(),
            name: "Спикер B".to_string(),
            channel: Some(0),
            matched: false,
            matched_name: None,
            matched_pack_id: None,
            matched_pack_name: None,
            similarity: 0.0,
            silhouette: 0.2,
            candidates: vec![("Спикер B".to_string(), 0.55)],
            embedding: Some(vec![1.0; mr_ml::EMBEDDING_DIM]),
            samples: vec![],
            utterances_count: 3,
            total_speech_sec: 5.0,
        },
    ];
    let mut utts = vec![];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(
        infos.len(),
        2,
        "конфликт кандидатов должен блокировать слияние"
    );
}

#[test]
fn split_by_word_pauses_splits_turn_exchanges() {
    let u = Utterance {
        start: 0.0,
        end: 8.0,
        speaker: "X".to_string(),
        channel: Some(0),
        text: "слово1 слово2 слово3 слово4".to_string(),
        diarized: false,
        silhouette: 0.0,
        words: vec![
            crate::transcribe::WordConf {
                w: "слово1".to_string(),
                conf: 0.99,
                needs_review: false,
                start_sec: 0.5,
                end_sec: 1.5,
            },
            crate::transcribe::WordConf {
                w: "слово2".to_string(),
                conf: 0.98,
                needs_review: false,
                start_sec: 1.6,
                end_sec: 2.5,
            },
            crate::transcribe::WordConf {
                w: "слово3".to_string(),
                conf: 0.97,
                needs_review: false,
                start_sec: 3.6,
                end_sec: 5.0,
            },
            crate::transcribe::WordConf {
                w: "слово4".to_string(),
                conf: 0.96,
                needs_review: false,
                start_sec: 5.2,
                end_sec: 6.5,
            },
        ],
        ctc_used: true,
    };
    let out = split_by_word_pauses(&[u], 0.8);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].text, "слово1 слово2");
    assert_eq!(out[1].text, "слово3 слово4");
    assert!((out[0].end - 3.05).abs() < 0.02);
    assert!((out[1].start - 3.05).abs() < 0.02);
}

#[test]
fn text_similarity_detects_containment_on_bleed() {
    let echo = "альфа бета гамма дельта эпсилон дзета";
    let original = "альфа бета гамма дельты эпсилон дзета эта тета йота каппа лямбда";
    assert!(text_similarity(echo, original) >= 0.85);
}

#[test]
fn speaker_change_cuts_detects_transition_window_stride_two() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let mix = vec![
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ];
    let wins: Vec<(f64, Vec<f64>)> = vec![(0.0, a), (1.0, mix), (2.0, b.clone()), (3.0, b)];
    let mut samples = vec![0.2f32; 16000 * 5];
    for x in samples[16000 * 25 / 10..16000 * 27 / 10].iter_mut() {
        *x = 0.0;
    }
    let cuts = speaker_change_cuts(&samples, 0.0, 5.0, &wins, &[]);
    assert_eq!(cuts.len(), 1);
    assert!((2.5..=2.7).contains(&cuts[0]), "{cuts:?}");
}

#[test]
fn local_region_keeps_local_run_and_skips_echo_only() {
    let mixed = echo_evidence(
        4.0,
        &[
            (0.0, 2.0, FrameClass::EchoOnly),
            (2.0, 4.0, FrameClass::LocalOnly),
        ],
    );
    let (start, end) =
        local_region(&mixed, 0.0, 4.0, MIN_LOCAL_EMBED_SEC).expect("локальная речь есть");
    assert!((start - 2.0).abs() < 0.05, "{start}");
    assert!((end - 4.0).abs() < 0.05, "{end}");

    let only_echo = echo_evidence(4.0, &[(0.0, 4.0, FrameClass::EchoOnly)]);
    assert!(local_region(&only_echo, 0.0, 4.0, MIN_LOCAL_EMBED_SEC).is_none());

    let too_short = echo_evidence(
        2.2,
        &[
            (0.0, 1.7, FrameClass::EchoOnly),
            (1.7, 2.2, FrameClass::LocalOnly),
        ],
    );
    assert!(local_region(&too_short, 0.0, 2.2, MIN_LOCAL_EMBED_SEC).is_none());
}

#[test]
fn split_window_region_rejects_echo_dominant_window() {
    let ev = echo_evidence(
        3.0,
        &[
            (0.0, 1.0, FrameClass::LocalOnly),
            (1.0, 3.0, FrameClass::EchoOnly),
        ],
    );
    let (start, end) = split_window_region(Some(&ev), 0.0, 1.6).expect("локальная половина окна");
    assert!(start < 0.05, "{start}");
    assert!((end - 1.0).abs() < 0.05, "{end}");
    assert!(split_window_region(Some(&ev), 1.0, 2.6).is_none());
    assert_eq!(split_window_region(None, 0.0, 1.6), Some((0.0, 1.6)));
}

#[test]
fn short_cluster_is_candidate_for_profile_match() {
    let label_of: HashMap<usize, usize> = HashMap::from([(0, 0), (1, 0), (2, 1), (3, 1)]);
    let embs = vec![
        Some(vec![1.0, 0.0]),
        Some(vec![1.0, 0.0]),
        Some(vec![0.0, 1.0]),
        None,
    ];
    let durs = vec![1.5, 1.5, 1.0, 4.0];
    let raw_clusters = vec![0usize, 1];
    let eligible = eligible_clusters(&raw_clusters, &label_of, &embs, &durs);
    assert_eq!(
        eligible.iter().map(|&(index, _)| index).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert!((eligible[0].1 - 3.0).abs() < 1e-9, "{eligible:?}");
    assert!((eligible[1].1 - 1.0).abs() < 1e-9, "{eligible:?}");

    let durs = vec![1.5, 1.5, 2.4, 4.0];
    assert_eq!(
        eligible_clusters(&raw_clusters, &label_of, &embs, &durs)
            .iter()
            .map(|&(index, _)| index)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
}

#[test]
fn short_name_requires_high_similarity_and_margin() {
    let mut candidate = mr_ml::ClusterMatch {
        matched: true,
        name: Some("A".to_string()),
        pack_id: None,
        pack_name: None,
        similarity: 0.96,
        margin: 0.06,
        candidates: vec![],
    };
    assert!(short_name_allowed(&candidate));
    candidate.similarity = 0.94;
    assert!(!short_name_allowed(&candidate));
    candidate.similarity = 0.96;
    candidate.margin = 0.04;
    assert!(!short_name_allowed(&candidate));
    candidate.margin = 0.06;
    candidate.matched = false;
    assert!(!short_name_allowed(&candidate));
}

#[test]
fn weak_short_reply_moves_to_matching_neighbors() {
    let utterances = vec![
        utt(0.0, 2.0, "A", 0, "нейтральная тестовая фраза"),
        utt(2.2, 2.7, "A", 0, "да"),
        utt(2.9, 5.0, "A", 0, "нейтральная тестовая фраза"),
        utt(10.0, 12.0, "A", 0, "нейтральная тестовая фраза"),
    ];
    let embs = vec![
        Some(vec![1.0, 0.0]),
        Some(vec![1.0, 0.0]),
        Some(vec![1.0, 0.0]),
        Some(vec![-1.0, 0.0]),
    ];
    let durs = vec![2.0, 0.5, 2.1, 2.0];
    let quals = vec![1.0; 4];
    let mut label_of: HashMap<usize, usize> = HashMap::from([(0, 0), (1, 1), (2, 0), (3, 1)]);
    reassign_short_replies(&utterances, &embs, &durs, &quals, &mut label_of);
    assert_eq!(label_of[&1], 0, "{label_of:?}");
}

#[test]
fn confident_short_reply_keeps_its_cluster() {
    let utterances = vec![
        utt(0.0, 2.0, "A", 0, "нейтральная тестовая фраза"),
        utt(2.2, 2.7, "A", 0, "да"),
        utt(2.9, 5.0, "A", 0, "нейтральная тестовая фраза"),
    ];
    let embs = vec![
        Some(vec![0.0, 1.0]),
        Some(vec![1.0, 0.0]),
        Some(vec![0.0, 1.0]),
    ];
    let durs = vec![2.0, 0.5, 2.1];
    let quals = vec![1.0; 3];
    let mut label_of: HashMap<usize, usize> = HashMap::from([(0, 0), (1, 1), (2, 0)]);
    reassign_short_replies(&utterances, &embs, &durs, &quals, &mut label_of);
    assert_eq!(label_of[&1], 1, "{label_of:?}");
}

#[test]
fn short_reply_with_different_neighbors_stays() {
    let utterances = vec![
        utt(0.0, 2.0, "A", 0, "нейтральная тестовая фраза"),
        utt(2.2, 2.7, "A", 0, "да"),
        utt(2.9, 5.0, "A", 0, "нейтральная тестовая фраза"),
        utt(10.0, 12.0, "A", 0, "нейтральная тестовая фраза"),
    ];
    let embs = vec![
        Some(vec![1.0, 0.0]),
        Some(vec![1.0, 0.0]),
        Some(vec![0.0, 1.0]),
        Some(vec![0.0, -1.0]),
    ];
    let durs = vec![2.0, 0.5, 2.1, 2.0];
    let quals = vec![1.0; 4];
    let mut label_of: HashMap<usize, usize> = HashMap::from([(0, 0), (1, 2), (2, 1), (3, 2)]);
    reassign_short_replies(&utterances, &embs, &durs, &quals, &mut label_of);
    assert_eq!(label_of[&1], 2, "{label_of:?}");
}

#[test]
fn strong_echo_suppresses_promotion_unless_local_speech_is_present() {
    let units = vec![
        AcoustUnit {
            utterance: 0,
            start: 0.0,
            end: 2.0,
            dur: 2.0,
            quality: 1.0,
            embedding: vec![1.0, 0.0],
        },
        AcoustUnit {
            utterance: 1,
            start: 4.0,
            end: 5.5,
            dur: 1.5,
            quality: 1.0,
            embedding: vec![0.0, 1.0],
        },
        AcoustUnit {
            utterance: 2,
            start: 8.0,
            end: 9.5,
            dur: 1.5,
            quality: 1.0,
            embedding: vec![-1.0, 0.0],
        },
    ];
    let mut ev = echo_evidence(
        10.0,
        &[
            (0.0, 2.0, FrameClass::LocalOnly),
            (4.0, 5.2, FrameClass::Uncertain),
            (5.2, 5.5, FrameClass::LocalOnly),
            (8.0, 8.6, FrameClass::LocalOnly),
            (8.6, 9.5, FrameClass::EchoOnly),
        ],
    );
    ev.prominence = crate::transcribe::echo::STRONG_PROMINENCE + 10.0;

    let mut labels = HashMap::from([(0usize, 0usize)]);
    let mut next = 1usize;
    let promoted = promote_unmatched_utterances(
        &units,
        &mut labels,
        &mut next,
        0,
        SpeechSupport::default(),
        Some(&ev),
    );
    assert_eq!(promoted, vec![1]);
    assert!(
        !labels.contains_key(&1),
        "остаточное эхо не должно выдвигаться"
    );
    assert_eq!(
        labels.get(&2),
        Some(&1),
        "локальная речь ≥0.25 должна оставаться"
    );

    ev.prominence = 12.0;
    let mut weak_labels = HashMap::from([(0usize, 0usize)]);
    let mut weak_next = 1usize;
    let weak_promoted = promote_unmatched_utterances(
        &units,
        &mut weak_labels,
        &mut weak_next,
        0,
        SpeechSupport::default(),
        Some(&ev),
    );
    assert_eq!(weak_promoted.len(), 2);
}

#[test]
fn promoted_single_utterance_survives_viterbi_smoothing_when_restored() {
    let utterances = vec![
        utt(0.0, 3.0, "Собеседник 1", 1, "альфа"),
        utt(3.1, 4.3, "Собеседник 2", 1, "бета"),
        utt(4.4, 8.0, "Собеседник 1", 1, "гамма"),
    ];
    let a = vec![1.0, 0.0];
    let b = vec![0.75, (1.0f64 - 0.75 * 0.75).sqrt()];
    let embs = vec![Some(a.clone()), Some(b), Some(a)];
    let durs = vec![3.0, 1.2, 3.6];
    let quals = vec![1.0; 3];
    let mut label_of = HashMap::from([(0usize, 0usize), (1, 1), (2, 0)]);
    let promoted_utts = promoted_utterances(&label_of, &HashSet::from([1usize]));
    smooth_labels_mono(&utterances, &embs, &durs, &quals, &mut label_of);
    assert_eq!(
        label_of[&1], 0,
        "сглаживание без восстановления подавляет одиночную реплику"
    );
    restore_protected(&mut label_of, &promoted_utts);
    assert_eq!(label_of[&1], 1);
}

#[test]
fn ch1_refinement_splits_at_aligned_pause_and_assigns_voice_run_speaker() {
    let mut u = utt(
        0.0,
        8.0,
        "Собеседник 1",
        1,
        "альфа бета гамма дельта эпсилон дзета",
    );
    u.words = vec![
        word("альфа", 0.2, 1.2),
        word("бета", 1.3, 2.5),
        word("гамма", 2.6, 3.9),
        word("дельта", 4.15, 5.2),
        word("эпсилон", 5.3, 6.5),
        word("дзета", 6.6, 7.8),
    ];
    u.ctc_used = true;
    let voice_timeline = vec![
        utt(0.0, 3.9, "Собеседник 1", 1, ""),
        utt(4.2, 8.0, "Собеседник 2", 1, ""),
    ];
    let mut utterances = vec![u];
    refine_ch1_utterances(
        &mut utterances,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(utterances.len(), 2, "{utterances:?}");
    assert_eq!(utterances[0].speaker, "Собеседник 1");
    assert_eq!(utterances[0].text, "альфа бета гамма");
    assert!((utterances[0].end - 4.03).abs() < 0.02, "{utterances:?}");
    assert_eq!(utterances[1].speaker, "Собеседник 2");
    assert_eq!(utterances[1].text, "дельта эпсилон дзета");
    assert_eq!(utterances[1].start, utterances[0].end);
}

#[test]
fn ch1_refinement_splits_at_lexical_boundary_with_turn_dash() {
    let mut u = utt(
        0.0,
        7.5,
        "Собеседник 1",
        1,
        "альфа бета гамма. — дельта эпсилон",
    );
    u.words = vec![
        word("альфа", 0.3, 1.1),
        word("бета", 1.2, 2.0),
        word("гамма.", 2.1, 3.2),
        word("—", 0.0, 0.0),
        word("дельта", 3.3, 5.0),
        word("эпсилон", 5.1, 7.2),
    ];
    u.ctc_used = true;
    let voice_timeline = vec![
        utt(0.0, 3.1, "Собеседник 1", 1, ""),
        utt(3.4, 7.5, "Собеседник 2", 1, ""),
    ];
    let mut utterances = vec![u];
    refine_ch1_utterances(
        &mut utterances,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(utterances.len(), 2, "{utterances:?}");
    assert_eq!(utterances[0].text, "альфа бета гамма.");
    assert_eq!(utterances[1].text, "— дельта эпсилон");
    assert_eq!(utterances[0].speaker, "Собеседник 1");
    assert_eq!(utterances[1].speaker, "Собеседник 2");
}

#[test]
fn ch1_refinement_skips_short_unaligned_and_boundary_runs() {
    let mut short = utt(0.0, 5.9, "Собеседник 1", 1, "альфа бета. гамма дельта");
    short.words = vec![
        word("альфа", 0.2, 1.2),
        word("бета.", 1.3, 2.8),
        word("гамма", 3.0, 4.2),
        word("дельта", 4.3, 5.8),
    ];
    let mut unaligned = utt(
        10.0,
        17.0,
        "Собеседник 1",
        1,
        "альфа бета гамма дельта эпсилон дзета",
    );
    unaligned.words = vec![
        word("альфа", 10.2, 11.2),
        word("бета", 11.24, 12.4),
        word("гамма", 12.44, 13.5),
        word("дельта", 13.54, 14.6),
        word("эпсилон", 14.64, 15.8),
        word("дзета", 15.84, 16.8),
    ];
    let mut near_edge = utt(20.0, 27.0, "Собеседник 1", 1, "альфа бета. гамма дельта");
    near_edge.words = vec![
        word("альфа", 20.2, 23.0),
        word("бета.", 23.1, 26.05),
        word("гамма", 26.25, 26.6),
        word("дельта", 26.65, 26.9),
    ];
    let voice_timeline = vec![
        utt(2.9, 5.9, "Собеседник 2", 1, ""),
        utt(13.5, 17.0, "Собеседник 2", 1, ""),
        utt(26.1, 27.0, "Собеседник 2", 1, ""),
    ];
    let mut utterances = vec![short, unaligned, near_edge];
    refine_ch1_utterances(
        &mut utterances,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(utterances.len(), 3, "{utterances:?}");
}

#[test]
fn ch1_refinement_requires_both_edges_aligned_for_interior_insertion() {
    let make_utt = |mid_punct: &str| {
        let mut u = utt(
            0.0,
            9.0,
            "Собеседник 1",
            1,
            &format!("альфа бета. гамма {mid_punct} эпсилон дзета"),
        );
        u.words = vec![
            word("альфа", 0.2, 1.4),
            word("бета.", 1.5, 2.9),
            word("гамма", 3.1, 4.3),
            word(mid_punct, 4.4, 5.6),
            word("эпсилон", 5.8, 7.2),
            word("дзета", 7.3, 8.8),
        ];
        u
    };
    let voice_timeline = vec![
        utt(0.0, 2.9, "Собеседник 1", 1, ""),
        utt(3.05, 5.7, "Собеседник 2", 1, ""),
        utt(5.8, 9.0, "Собеседник 1", 1, ""),
    ];
    let mut aligned = vec![make_utt("дельта?")];
    refine_ch1_utterances(
        &mut aligned,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(aligned.len(), 3, "{aligned:?}");
    assert_eq!(aligned[0].speaker, "Собеседник 1");
    assert_eq!(aligned[1].speaker, "Собеседник 2");
    assert_eq!(aligned[1].text, "гамма дельта?");
    assert_eq!(aligned[2].speaker, "Собеседник 1");

    let mut unaligned_end = vec![utt(
        0.0,
        9.0,
        "Собеседник 1",
        1,
        "альфа бета. гамма дельта эпсилон дзета",
    )];
    unaligned_end[0].words = vec![
        word("альфа", 0.2, 1.4),
        word("бета.", 1.5, 2.9),
        word("гамма", 3.1, 4.3),
        word("дельта", 4.4, 5.6),
        word("эпсилон", 5.64, 7.2),
        word("дзета", 7.25, 8.8),
    ];
    refine_ch1_utterances(
        &mut unaligned_end,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(unaligned_end.len(), 1, "{unaligned_end:?}");
}

#[test]
fn ch1_refinement_clamps_untimed_dash_in_large_gap_to_part_bounds() {
    let mut u = utt(0.0, 8.5, "Собеседник 1", 1, "альфа бета. — гамма дельта");
    u.words = vec![
        word("альфа", 0.2, 1.0),
        word("бета.", 1.1, 2.0),
        word("—", 0.0, 0.0),
        word("гамма", 4.0, 6.0),
        word("дельта", 6.1, 8.2),
    ];
    u.ctc_used = true;
    let voice_timeline = vec![
        utt(0.0, 2.0, "Собеседник 1", 1, ""),
        utt(3.2, 8.5, "Собеседник 2", 1, ""),
    ];
    let mut utterances = vec![u];
    refine_ch1_utterances(
        &mut utterances,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(utterances.len(), 2, "{utterances:?}");
    assert_eq!(utterances[0].text, "альфа бета.");
    assert_eq!(utterances[1].text, "— гамма дельта");
    let dash = &utterances[1].words[0];
    assert_eq!(dash.w, "—");
    assert!(
        dash.start_sec >= utterances[1].start && dash.end_sec <= utterances[1].end,
        "dash bounds {:?}..{:?} must stay inside part {:?}..{:?}",
        dash.start_sec,
        dash.end_sec,
        utterances[1].start,
        utterances[1].end
    );
    assert!(dash.start_sec >= utterances[1].start - 0.5 && dash.end_sec <= utterances[1].end + 0.5);
    assert!(dash.start_sec <= dash.end_sec);
}

#[test]
fn ch1_refinement_verifier_rejects_unconfirmed_split_and_allows_confirmed() {
    let mut base = utt(0.0, 6.5, "Собеседник 1", 1, "альфа бета гамма. дельта");
    base.words = vec![
        word("альфа", 0.2, 1.5),
        word("бета", 1.6, 3.0),
        word("гамма.", 3.1, 5.0),
        word("дельта", 5.4, 6.4),
    ];
    let voice_timeline = vec![
        utt(0.0, 5.2, "Собеседник 1", 1, ""),
        utt(5.3, 6.5, "Собеседник 2", 1, ""),
    ];

    let mut rejected = vec![base.clone()];
    refine_ch1_with_verifier(
        &mut rejected,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
        &mut |_, _, _, _| false,
    );
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].speaker, "Собеседник 1");

    let mut confirmed = vec![base];
    refine_ch1_with_verifier(
        &mut confirmed,
        &voice_timeline,
        &HashMap::new(),
        &HashMap::new(),
        &mut |_, _, _, _| true,
    );
    assert_eq!(confirmed.len(), 2);
    assert_eq!(confirmed[0].speaker, "Собеседник 1");
    assert_eq!(confirmed[1].speaker, "Собеседник 2");
}

#[test]
fn reconcile_channels_blocks_transitive_merge_of_two_ch1_speakers_via_ch0_echo() {
    // Два безымянных кластера ch1 (разные голоса) и общий эхо-кластер ch0:
    // DSU не должен склеивать обоих удалённых участников в одного через ch0.
    let mut infos = vec![
        info("Собеседник 1", 1, vec![1.0, 0.0, 0.0], 5, 10.0),
        info("Собеседник 2", 1, vec![0.0, 1.0, 0.0], 5, 10.0),
        info("Участник 2", 0, vec![0.8, 0.6, 0.0], 2, 3.0),
    ];
    let mut utts = vec![
        utt(0.0, 2.0, "Собеседник 1", 1, "первая фраза эхо"),
        utt(0.1, 2.0, "Участник 2", 0, "первая фраза эхо"),
        utt(5.0, 7.0, "Собеседник 2", 1, "вторая фраза эхо"),
        utt(5.1, 7.0, "Участник 2", 0, "вторая фраза эхо"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 2, "{infos:?}");
    assert_ne!(utts[0].speaker, utts[2].speaker);
    // Более близкий по косинусу (0.8 vs 0.6) забирает эхо-кластер.
    assert_eq!(utts[1].speaker, "Собеседник 1");
}

#[test]
fn reconcile_channels_short_rule_never_renames_large_mic_cluster_to_short_remote() {
    let mut infos = vec![
        info("Я", 0, vec![1.0, 0.0], 10, 20.0),
        info("Собеседник 1", 1, vec![0.0, 1.0], 1, 2.0),
    ];
    let mut utts = vec![
        utt(0.0, 2.0, "Я", 0, "короткая общая фраза"),
        utt(0.1, 2.0, "Собеседник 1", 1, "короткая общая фраза"),
    ];
    reconcile_channels(&mut infos, &mut utts);
    assert_eq!(infos.len(), 2, "{infos:?}");
    assert_eq!(utts[0].speaker, "Я");
    assert_eq!(utts[1].speaker, "Собеседник 1");
}

#[test]
fn move_boundary_words_updates_next_start() {
    let mut u1 = utt(0.0, 2.0, "A", 0, "раз два три");
    u1.words = vec![
        word("раз", 0.2, 0.6),
        word("два", 0.7, 1.2),
        word("три", 1.96, 2.0),
    ];
    let mut u2 = utt(2.1, 3.0, "A", 0, "четыре");
    u2.words = vec![word("четыре", 2.15, 2.8)];
    let mut utts = vec![u1, u2];
    move_boundary_words(&mut utts);
    assert_eq!(utts.len(), 2);
    assert!(utts[1].start <= utts[1].words[0].start_sec);
}

#[test]
fn split_at_pauses_finds_pauses_in_mostly_silent_channel() {
    // 90% канала — тишина, но внутри 4-секундной реплики есть пауза 0.4 с:
    // квантили считаются по репликам, а не по тишине всего канала.
    let mut samples = vec![0.0f32; 16_000 * 40];
    let fill_tone = |buf: &mut [f32], start_s: f64, end_s: f64| {
        let lo = (start_s * 16_000.0) as usize;
        let hi = (end_s * 16_000.0) as usize;
        for (i, slot) in buf[lo..hi].iter_mut().enumerate() {
            *slot = 0.2 * (std::f32::consts::TAU * 300.0 * i as f32 / 16_000.0).sin();
        }
    };
    fill_tone(&mut samples, 10.0, 11.8);
    fill_tone(&mut samples, 12.2, 14.0);
    let utts = vec![utt(10.0, 14.0, "Собеседник 1", 1, "первая вторая")];
    let split = split_at_pauses(&samples, &utts);
    assert_eq!(split.len(), 2, "{split:?}");
    assert!((split[0].end - 12.0).abs() < 0.35, "{split:?}");
}
