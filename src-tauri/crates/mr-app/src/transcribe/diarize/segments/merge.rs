use std::collections::{HashMap, HashSet};

use mr_ml::SpeakerProfile;

use crate::transcribe::Utterance;

use super::labels::switch_penalty;

/// Длинную речь режем по минимуму вероятности VAD во второй половине окна.
pub(crate) fn group_segments(
    segments: &[(f32, f32)],
    probs: &[f32],
    max_dur: f32,
    gap: f32,
    ch: i32,
) -> Vec<(f32, f32)> {
    let win_sec = mr_ml::onnx::vad::VAD_WINDOW as f32 / 16000.0;
    let input_segments = segments.len();
    let mut cuts = 0usize;
    let mut norm: Vec<(f32, f32)> = Vec::new();
    for &(s, e) in segments {
        if e - s <= max_dur {
            norm.push((s, e));
            continue;
        }
        if probs.is_empty() {
            let n = ((e - s) / max_dur).ceil() as usize;
            let step = (e - s) / n as f32;
            for i in 0..n {
                norm.push((s + i as f32 * step, s + (i + 1) as f32 * step));
            }
            continue;
        }
        let mut a = s;
        while e - a > max_dur {
            let window_end = a + max_dur;
            let mid = a + max_dur / 2.0;
            let lo = (mid / win_sec).ceil().max(0.0) as usize;
            let hi = ((window_end / win_sec).ceil().max(0.0) as usize).min(probs.len());
            let cut = (lo..hi)
                .min_by(|&i, &j| probs[i].total_cmp(&probs[j]))
                .map(|i| i as f32 * win_sec)
                .filter(|&cut| cut > a && cut <= window_end)
                .unwrap_or(window_end);
            cuts += 1;
            log::trace!(
                target: "vad",
                ch = ch,
                at_sec = cut,
                p = probs.get((cut / win_sec) as usize).copied().unwrap_or(0.0),
                window_end_sec = window_end;
                "ev=vad.group.cut"
            );
            norm.push((a, cut));
            a = cut;
        }
        norm.push((a, e));
    }
    let mut grouped: Vec<(f32, f32)> = Vec::new();
    for (s, e) in norm {
        if let Some(last) = grouped.last_mut() {
            if e - last.0 <= max_dur && s - last.1 <= gap {
                last.1 = e;
                continue;
            }
        }
        grouped.push((s, e));
    }
    log::debug!(
        target: "vad",
        ch = ch,
        in_segs = input_segments,
        out_groups = grouped.len(),
        cuts = cuts;
        "ev=vad.group"
    );
    grouped
}

/// Без эхо-доказательств склейка ведёт себя как раньше: вызовы, где пара
/// каналов недоступна, не должны менять результат.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn merge_quiet_short(
    samples: &[f32],
    groups: Vec<(f32, f32)>,
    ch: i32,
) -> Vec<(f32, f32)> {
    merge_quiet_short_with_evidence(samples, groups, ch, None, None)
}

/// Доля эхо-кадров, начиная с которой фрагмент считается эхом и не склеивается
/// с локальной речью: иначе ASR получает окно «локальная фраза + чужое эхо».
const ECHO_MERGE_BLOCK: f32 = 0.5;

pub(crate) fn merge_quiet_short_with_evidence(
    samples: &[f32],
    groups: Vec<(f32, f32)>,
    ch: i32,
    evidence: Option<&crate::transcribe::echo::EchoEvidence>,
    system: Option<&[f32]>,
) -> Vec<(f32, f32)> {
    const SHORT_SEC: f32 = 2.0;
    const MAX_MERGED_SEC: f32 = 12.0;
    const QUIET_RMS: f32 = 0.02;
    const MAX_GAP_SEC: f32 = 3.0;
    let rms = |a: f32, b: f32| -> f32 {
        let lo = (a.max(0.0) * 16000.0) as usize;
        let hi = ((b.max(0.0) * 16000.0) as usize).min(samples.len());
        if hi <= lo {
            return 0.0;
        }
        let s = &samples[lo..hi];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    let system_rms = |a: f32, b: f32| -> f32 {
        let Some(system) = system else {
            return 0.0;
        };
        let lo = (a.max(0.0) * 16000.0) as usize;
        let hi = ((b.max(0.0) * 16000.0) as usize).min(system.len());
        if hi <= lo {
            return 0.0;
        }
        let s = &system[lo..hi];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    let echo_blocks = |a: (f32, f32), b: (f32, f32)| -> bool {
        let Some(evidence) = evidence else {
            return false;
        };
        let head = crate::transcribe::echo::segment_profile(evidence, a.0 as f64, a.1 as f64);
        let tail = crate::transcribe::echo::segment_profile(evidence, b.0 as f64, b.1 as f64);
        // Уровень системы против микрофона: тихий микрофон при громкой системе — это эхо.
        let level_echo = |span: (f32, f32)| {
            let sys_level = system_rms(span.0, span.1);
            let mic_level = rms(span.0, span.1);
            sys_level > 0.005 && mic_level <= evidence.gain * sys_level * 1.5
        };
        if level_echo(a) || level_echo(b) || (b.0 > a.1 && level_echo((a.1, b.0))) {
            return true;
        }
        head.echo_only >= ECHO_MERGE_BLOCK
            || tail.echo_only >= ECHO_MERGE_BLOCK
            || (b.0 > a.1
                && crate::transcribe::echo::segment_profile(evidence, a.1 as f64, b.0 as f64)
                    .echo_only
                    >= ECHO_MERGE_BLOCK)
    };
    let mut out: Vec<(f32, f32)> = Vec::new();
    let mut merged = 0usize;
    for (s, e) in groups {
        if let Some(last) = out.last_mut() {
            let gap = s - last.1;
            let incoming_short = e - s <= SHORT_SEC;
            let rms_a = rms(last.0, last.1);
            let rms_b = rms(s, e);
            let blocked = echo_blocks(*last, (s, e));
            if incoming_short
                && (0.0..=MAX_GAP_SEC).contains(&gap)
                && e - last.0 <= MAX_MERGED_SEC
                && rms_a < QUIET_RMS
                && rms_b < QUIET_RMS
                && !blocked
            {
                log::debug!(
                    target: "vad",
                    ch = ch,
                    merged = merged + 1,
                    gap_sec = gap,
                    rms_a = rms_a,
                    rms_b = rms_b;
                    "ev=vad.merge_quiet"
                );
                merged += 1;
                last.1 = e;
                continue;
            }
            if blocked && incoming_short && (0.0..=MAX_GAP_SEC).contains(&gap) {
                log::debug!(
                    target: "vad",
                    ch = ch,
                    gap_sec = gap,
                    at_sec = s;
                    "ev=vad.merge_echo_skip"
                );
            }
        }
        out.push((s, e));
    }
    out
}

pub(crate) fn short_join_threshold(dur: f64) -> f64 {
    const MIN: f64 = 0.36;
    const SPAN: f64 = 0.12;
    MIN + SPAN * ((dur - 0.4) / 0.8).clamp(0.0, 1.0)
}

/// Кластеры короче 2% речи — обычно шумные короткие эмбеддинги. Клеим их
/// к ближайшему крупному голосу (косинус ≥ 0.45), а непохожие оставляем:
/// это может быть третий человек, сказавший пару слов.
pub(crate) fn merge_tiny_clusters(
    vecs: &[Vec<f64>],
    labels: &mut [usize],
    weights: &[f64],
    min_keep: usize,
    profiles: &[SpeakerProfile],
    constraints: &[(usize, usize)],
) {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return;
    }
    let min_sec = (total * 0.02).clamp(2.0, 10.0);
    // Кластер с хорошей обособленностью — отдельный голос, пусть и короткий.
    let scores = mr_ml::cluster::silhouette_scores(vecs, labels);
    let mut silhouettes: HashMap<usize, (f64, f64)> = HashMap::new();
    for (i, &label) in labels.iter().enumerate() {
        let entry = silhouettes.entry(label).or_default();
        entry.0 += scores.get(i).copied().unwrap_or(0.0) * weights[i];
        entry.1 += weights[i];
    }
    let well_separated = |label: usize| {
        silhouettes
            .get(&label)
            .is_some_and(|&(sum, weight)| weight > 0.0 && sum / weight > 0.3)
    };
    let mut protected: HashSet<usize> = HashSet::new();
    loop {
        let mut totals: HashMap<usize, f64> = HashMap::new();
        for (i, l) in labels.iter().enumerate() {
            *totals.entry(*l).or_default() += weights[i];
        }
        if totals.len() <= min_keep.max(1) {
            return;
        }
        let Some((&small, _)) = totals
            .iter()
            .filter(|(l, &t)| t < min_sec && !protected.contains(l) && !well_separated(**l))
            // Тай-брейк по номеру кластера: порядок обхода `HashMap` в процессе
            // случаен, и без него результат зависел бы от запуска.
            .min_by(|a, b| a.1.total_cmp(b.1).then_with(|| a.0.cmp(b.0)))
        else {
            return;
        };
        let centroid = |l: usize| {
            let members: Vec<usize> = labels
                .iter()
                .enumerate()
                .filter(|(_, &x)| x == l)
                .map(|(i, _)| i)
                .collect();
            let vs: Vec<Vec<f64>> = members.iter().map(|&i| vecs[i].clone()).collect();
            let ws: Vec<f64> = members.iter().map(|&i| weights[i]).collect();
            mr_ml::weighted_centroid(&vs, &ws)
        };
        let cs = centroid(small);
        if profiles
            .iter()
            .any(|p| mr_ml::cosine_similarity(&cs, &p.embedding) >= mr_ml::DEFAULT_MATCH_THRESHOLD)
        {
            protected.insert(small);
            continue;
        }
        let mut best = (usize::MAX, -2.0f64);
        for &l in totals.keys() {
            if l == small
                || constraints.iter().any(|&(a, b)| {
                    (labels[a] == small && labels[b] == l) || (labels[b] == small && labels[a] == l)
                })
            {
                continue;
            }
            let sim = mr_ml::cosine_similarity(&cs, &centroid(l));
            if sim > best.1 || (sim == best.1 && l < best.0) {
                best = (l, sim);
            }
        }
        if best.1 < 0.45 {
            protected.insert(small);
            continue;
        }
        log::debug!(
            target: "diar",
            from = small,
            to = best.0,
            similarity = best.1,
            n = totals.get(&small).copied().unwrap_or(0.0);
            "ev=diar.merge_tiny"
        );
        for l in labels.iter_mut() {
            if *l == small {
                *l = best.0;
            }
        }
    }
}

/// Сглаживание меток по времени (только моно): одиночный шумный эмбеддинг
/// не должен переключать спикера, если соседние куски уверенно относятся
/// к другому голосу. Витерби по сходству с центроидами + штраф за смену.
pub(crate) fn smooth_labels_mono(
    utterances: &[Utterance],
    embs: &[Option<Vec<f64>>],
    durs: &[f64],
    quals: &[f64],
    label_of: &mut HashMap<usize, usize>,
) {
    let mut order: Vec<usize> = (0..utterances.len()).collect();
    order.sort_by(|&a, &b| utterances[a].start.total_cmp(&utterances[b].start));
    for _ in 0..2 {
        let mut ids: Vec<usize> = label_of.values().copied().collect();
        ids.sort();
        ids.dedup();
        if ids.len() < 2 {
            return;
        }
        let centroids: Vec<Vec<f64>> = ids
            .iter()
            .map(|&l| {
                let mut members: Vec<usize> = label_of
                    .iter()
                    .filter(|(_, &x)| x == l)
                    .map(|(&i, _)| i)
                    .filter(|&i| embs[i].is_some())
                    .collect();
                members.sort_unstable();
                let vs: Vec<Vec<f64>> = members.iter().map(|&i| embs[i].clone().unwrap()).collect();
                let ws: Vec<f64> = members
                    .iter()
                    .map(|&i| durs[i].min(8.0) * quals[i])
                    .collect();
                if vs.is_empty() {
                    vec![]
                } else {
                    mr_ml::weighted_centroid(&vs, &ws)
                }
            })
            .collect();
        let mut back: Vec<Vec<usize>> = Vec::with_capacity(order.len());
        let mut prev = vec![0.0f64; ids.len()];
        for (pos, &i) in order.iter().enumerate() {
            let em: Vec<f64> = match &embs[i] {
                Some(e) => centroids
                    .iter()
                    .map(|c| mr_ml::cosine_similarity(e, c))
                    .collect(),
                None => vec![0.0; ids.len()],
            };
            if pos == 0 {
                prev = em;
                back.push(vec![0; ids.len()]);
                continue;
            }
            let gap = (utterances[i].start - utterances[order[pos - 1]].end).max(0.0);
            let penalty = switch_penalty(gap);
            let (mut b1, mut b1s, mut b2, mut b2s) =
                (f64::NEG_INFINITY, 0usize, f64::NEG_INFINITY, 0usize);
            for (s, &v) in prev.iter().enumerate() {
                if v > b1 {
                    b2 = b1;
                    b2s = b1s;
                    b1 = v;
                    b1s = s;
                } else if v > b2 {
                    b2 = v;
                    b2s = s;
                }
            }
            let mut cur = vec![0.0f64; ids.len()];
            let mut bp = vec![0usize; ids.len()];
            for s in 0..ids.len() {
                let (other, other_s) = if s == b1s { (b2, b2s) } else { (b1, b1s) };
                if prev[s] >= other - penalty {
                    cur[s] = prev[s] + em[s];
                    bp[s] = s;
                } else {
                    cur[s] = other + em[s] - penalty;
                    bp[s] = other_s;
                }
            }
            prev = cur;
            back.push(bp);
        }
        let mut state = (0..ids.len())
            .max_by(|&a, &b| prev[a].total_cmp(&prev[b]))
            .unwrap_or(0);
        for pos in (0..order.len()).rev() {
            label_of.insert(order[pos], ids[state]);
            state = back[pos][state];
        }
    }
}
