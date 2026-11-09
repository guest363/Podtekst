use std::collections::BTreeMap;

use super::{Utterance, WordConf};

#[derive(Debug)]
struct DiarSegment {
    start: f64,
    end: f64,
    speaker: String,
    silhouette: f64,
}

#[derive(Debug)]
struct Node {
    center: f64,
    by_start: Vec<usize>,
    by_end: Vec<usize>,
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
}

impl Node {
    fn build(segments: &[DiarSegment], indices: Vec<usize>) -> Option<Box<Self>> {
        if indices.is_empty() {
            return None;
        }
        let mut endpoints = Vec::with_capacity(indices.len() * 2);
        for &i in &indices {
            endpoints.push(segments[i].start);
            endpoints.push(segments[i].end);
        }
        endpoints.sort_by(f64::total_cmp);
        let center = endpoints[endpoints.len() / 2];
        let mut crossing = Vec::new();
        let mut left = Vec::new();
        let mut right = Vec::new();
        for i in indices {
            let s = &segments[i];
            if s.start <= center && s.end >= center {
                crossing.push(i);
            } else if s.end < center {
                left.push(i);
            } else {
                right.push(i);
            }
        }
        let mut by_start = crossing.clone();
        by_start.sort_by(|&a, &b| segments[a].start.total_cmp(&segments[b].start));
        let mut by_end = crossing;
        by_end.sort_by(|&a, &b| segments[a].end.total_cmp(&segments[b].end));
        Some(Box::new(Self {
            center,
            by_start,
            by_end,
            left: Self::build(segments, left),
            right: Self::build(segments, right),
        }))
    }

    fn query<'a>(
        &self,
        segments: &'a [DiarSegment],
        start: f64,
        end: f64,
        visit: &mut impl FnMut(&'a DiarSegment),
    ) {
        if end < self.center {
            let stop = self.by_start.partition_point(|&i| segments[i].start <= end);
            for &i in &self.by_start[..stop] {
                visit(&segments[i]);
            }
            if let Some(left) = &self.left {
                left.query(segments, start, end, visit);
            }
        } else if start > self.center {
            let from = self.by_end.partition_point(|&i| segments[i].end < start);
            for &i in &self.by_end[from..] {
                visit(&segments[i]);
            }
            if let Some(right) = &self.right {
                right.query(segments, start, end, visit);
            }
        } else {
            for &i in &self.by_start {
                visit(&segments[i]);
            }
            if let Some(left) = &self.left {
                left.query(segments, start, end, visit);
            }
            if let Some(right) = &self.right {
                right.query(segments, start, end, visit);
            }
        }
    }
}

/// Интервальное дерево диаризации: построение O(N log N), запрос O(log N + K).
#[derive(Debug)]
pub(crate) struct DiarIntervalTree {
    segments: Vec<DiarSegment>,
    edges: Vec<f64>,
    root: Option<Box<Node>>,
}

impl DiarIntervalTree {
    pub(crate) fn new(utterances: &[Utterance]) -> Self {
        let segments: Vec<DiarSegment> = utterances
            .iter()
            .filter(|u| {
                u.start.is_finite() && u.end.is_finite() && u.end > u.start && !u.speaker.is_empty()
            })
            .map(|u| DiarSegment {
                start: u.start,
                end: u.end,
                speaker: u.speaker.clone(),
                silhouette: u.silhouette,
            })
            .collect();
        let root = Node::build(&segments, (0..segments.len()).collect());
        let mut edges = Vec::with_capacity(segments.len() * 2);
        for segment in &segments {
            edges.push(segment.start);
            edges.push(segment.end);
        }
        edges.sort_by(f64::total_cmp);
        Self {
            segments,
            edges,
            root,
        }
    }

    pub(crate) fn assign_dominant_speaker(&self, start: f64, end: f64) -> Option<(String, f64)> {
        let root = self.root.as_ref()?;
        let mut scores: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
        root.query(&self.segments, start, end, &mut |s| {
            let overlap = (end.min(s.end) - start.max(s.start)).max(0.0);
            if overlap <= 0.0 {
                return;
            }
            let score = scores.entry(&s.speaker).or_default();
            score.0 += overlap;
            score.1 += s.silhouette * overlap;
        });
        scores
            .into_iter()
            .max_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
            .map(|(speaker, (area, weighted))| {
                (speaker.to_string(), weighted / area.max(f64::EPSILON))
            })
    }

    fn snap_to_pause(&self, time: f64, window: f64) -> Option<f64> {
        let at = self.edges.partition_point(|&edge| edge < time);
        let mut best: Option<(f64, f64)> = None;
        if at > 0 {
            let edge = self.edges[at - 1];
            let distance = (edge - time).abs();
            if distance <= window {
                best = Some((distance, edge));
            }
        }
        if let Some(&edge) = self.edges.get(at) {
            let distance = (edge - time).abs();
            if distance <= window && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, edge));
            }
        }
        best.map(|(_, edge)| edge)
    }
}

pub(crate) fn resolve_word_bounds(
    words: &[&str],
    raw_words: &[WordConf],
    utt_start: f64,
    utt_end: f64,
) -> Vec<(f64, f64)> {
    let word_count = words.len();
    if word_count == 0 {
        return Vec::new();
    }
    let timed: Vec<Option<(f64, f64)>> = (0..word_count)
        .map(|i| {
            raw_words
                .get(i)
                .filter(|w| w.end_sec > w.start_sec)
                .map(|w| (w.start_sec, w.end_sec))
        })
        .collect();

    if timed.iter().any(Option::is_some) {
        let mut resolved: Vec<(f64, f64)> = vec![(0.0, 0.0); word_count];
        let mut i = 0;
        while i < word_count {
            if let Some(span) = timed[i] {
                resolved[i] = span;
                i += 1;
            } else {
                let start_idx = i;
                while i < word_count && timed[i].is_none() {
                    i += 1;
                }
                let end_idx = i;
                let left = if start_idx > 0 {
                    resolved[start_idx - 1].1
                } else {
                    utt_start
                };
                let right = if end_idx < word_count {
                    timed[end_idx].expect("завершается на Some").0
                } else {
                    utt_end
                };
                let count = (end_idx - start_idx) as f64;
                let span_len = (right - left).max(0.02 * count);
                let weights: Vec<f64> = (start_idx..end_idx)
                    .map(|k| words[k].chars().count() as f64 + 1.0)
                    .collect();
                let tot_w: f64 = weights.iter().sum::<f64>().max(f64::EPSILON);
                let mut cur = left;
                for (w_i, &w) in weights.iter().enumerate() {
                    let k = start_idx + w_i;
                    let dur = span_len * w / tot_w;
                    resolved[k] = (cur, cur + dur);
                    cur += dur;
                }
            }
        }
        resolved
    } else {
        let weights: Vec<f64> = words
            .iter()
            .map(|word| word.chars().count() as f64 + 1.0)
            .collect();
        let total: f64 = weights.iter().sum::<f64>().max(f64::EPSILON);
        let step = ((utt_end - utt_start) / word_count as f64).min(0.02);
        let mut bounds = Vec::with_capacity(word_count + 1);
        bounds.push(utt_start);
        let mut passed = 0.0;
        for (i, weight) in weights.iter().take(word_count - 1).enumerate() {
            passed += weight;
            let raw = utt_start + (utt_end - utt_start) * passed / total;
            let previous = *bounds.last().unwrap_or(&utt_start);
            let upper = (utt_end - (word_count - 1 - i) as f64 * step).max(utt_start);
            let lower = (previous + step).min(upper);
            bounds.push(raw.clamp(lower, upper));
        }
        bounds.push(utt_end);
        bounds.windows(2).map(|w| (w[0], w[1])).collect()
    }
}

pub(crate) fn split_utterance_by_word_speakers(
    utterance: &Utterance,
    tree: &DiarIntervalTree,
) -> Vec<Utterance> {
    let words: Vec<&str> = utterance.text.split_whitespace().collect();
    if words.is_empty() || utterance.end <= utterance.start || tree.root.is_none() {
        return vec![utterance.clone()];
    }
    let word_count = words.len();
    let timed: Vec<Option<(f64, f64)>> = (0..word_count)
        .map(|i| {
            utterance
                .words
                .get(i)
                .filter(|w| w.end_sec > w.start_sec)
                .map(|w| (w.start_sec, w.end_sec))
        })
        .collect();

    let bounds: Vec<(f64, f64)> = if timed.iter().any(Option::is_some) {
        resolve_word_bounds(&words, &utterance.words, utterance.start, utterance.end)
    } else {
        let weights: Vec<f64> = words
            .iter()
            .map(|word| word.chars().count() as f64 + 1.0)
            .collect();
        let total: f64 = weights.iter().sum::<f64>().max(f64::EPSILON);
        // 20 мс — минимум, чтобы assign_dominant_speaker видел пересечение;
        // иначе кумулятивный пол даёт слова нулевой длины.
        let step = ((utterance.end - utterance.start) / word_count as f64).min(0.02);
        let mut bounds = Vec::with_capacity(word_count + 1);
        bounds.push(utterance.start);
        let mut passed = 0.0;
        for (i, weight) in weights.iter().take(word_count - 1).enumerate() {
            passed += weight;
            let raw = utterance.start + (utterance.end - utterance.start) * passed / total;
            let snapped = tree.snap_to_pause(raw, 0.15).unwrap_or(raw);
            let previous = *bounds.last().unwrap_or(&utterance.start);
            // Резерв на оставшиеся слова: ранняя граница не должна съесть их время.
            let upper = (utterance.end - (word_count - 1 - i) as f64 * step).max(utterance.start);
            let lower = (previous + step).min(upper);
            let chosen = if snapped > lower { snapped } else { raw };
            bounds.push(chosen.clamp(lower, upper));
        }
        bounds.push(utterance.end);
        bounds.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let mut assigned = Vec::with_capacity(word_count);
    for (i, word) in words.into_iter().enumerate() {
        let (start, end) = bounds[i];
        let (speaker, silhouette) = tree
            .assign_dominant_speaker(start, end)
            .unwrap_or_else(|| (utterance.speaker.clone(), utterance.silhouette));
        let mut wc = utterance.words.get(i).cloned();
        if let Some(ref mut w) = wc {
            if w.end_sec <= w.start_sec {
                w.start_sec = (start * 1000.0).round() / 1000.0;
                w.end_sec = (end * 1000.0).round() / 1000.0;
            }
        }
        assigned.push((word, start, end, speaker, silhouette, wc));
    }

    let mut i = 0;
    while i < assigned.len() {
        let start = i;
        let speaker = assigned[i].3.clone();
        while i < assigned.len() && assigned[i].3 == speaker {
            i += 1;
        }
        let duration = assigned[i - 1].2 - assigned[start].1;
        if duration < 0.5 && start > 0 && assigned[start - 1].3 != speaker {
            let gap = assigned[start].1 - assigned[start - 1].2;
            let sentence_end = assigned[start - 1].0.ends_with(['.', '!', '?']);
            if gap < 0.15 && !sentence_end {
                let previous = assigned[start - 1].3.clone();
                for word in &mut assigned[start..i] {
                    word.3 = previous.clone();
                }
            }
        }
    }
    let mut out: Vec<Utterance> = Vec::new();
    for (word, start, end, speaker, silhouette, wc) in assigned {
        if let Some(last) = out.last_mut() {
            if last.speaker == speaker {
                let old_dur = (last.end - last.start).max(0.0);
                let new_dur = (end - start).max(0.0);
                last.silhouette = (last.silhouette * old_dur + silhouette * new_dur)
                    / (old_dur + new_dur).max(f64::EPSILON);
                last.end = end;
                last.text.push(' ');
                last.text.push_str(word);
                if let Some(w) = wc {
                    last.words.push(w);
                }
                continue;
            }
        }
        out.push(Utterance {
            start,
            end,
            speaker,
            channel: utterance.channel,
            text: word.to_string(),
            diarized: true,
            silhouette,
            words: wc.map(|w| vec![w]).unwrap_or_default(),
            ctc_used: utterance.ctc_used,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::WordConf;

    fn timed_word(text: &str, start: f64, end: f64) -> WordConf {
        WordConf {
            w: text.to_string(),
            conf: 1.0,
            needs_review: false,
            start_sec: start,
            end_sec: end,
        }
    }

    fn utt(start: f64, end: f64, speaker: &str, text: &str) -> Utterance {
        Utterance {
            start,
            end,
            speaker: speaker.to_string(),
            channel: None,
            text: text.to_string(),
            diarized: true,
            silhouette: 0.5,
            words: Vec::new(),
            ctc_used: false,
        }
    }

    #[test]
    fn dominant_speaker_uses_exact_overlap_area() {
        let tree = DiarIntervalTree::new(&[
            utt(0.0, 4.0, "A", "a"),
            utt(2.0, 8.0, "B", "b"),
            utt(10.0, 20.0, "C", "c"),
            utt(12.0, 14.0, "D", "d"),
        ]);
        assert_eq!(tree.assign_dominant_speaker(3.0, 5.0).unwrap().0, "B");
        assert_eq!(tree.assign_dominant_speaker(0.0, 1.0).unwrap().0, "A");
        assert_eq!(tree.assign_dominant_speaker(12.5, 13.0).unwrap().0, "D");
        assert!(tree.assign_dominant_speaker(30.0, 31.0).is_none());
    }

    #[test]
    fn utterance_splits_only_at_word_speaker_changes() {
        let tree = DiarIntervalTree::new(&[utt(0.0, 2.0, "A", "a"), utt(2.0, 4.0, "B", "b")]);
        let source = utt(0.0, 4.0, "unknown", "one two, three four");
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "one two,");
        assert_eq!(out[0].speaker, "A");
        assert_eq!(out[1].text, "three four");
        assert_eq!(out[1].speaker, "B");
        assert_eq!(
            out.iter()
                .map(|u| u.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            source.text
        );
        assert_eq!(out[0].end, out[1].start);
    }

    #[test]
    fn short_reply_after_pause_keeps_its_speaker() {
        let tree = DiarIntervalTree::new(&[
            utt(0.0, 4.0, "A", "a"),
            utt(4.25, 4.55, "B", "b"),
            utt(4.8, 6.2, "A", "a"),
        ]);
        let mut source = utt(
            0.0,
            6.2,
            "unknown",
            "первая фраза говорит да продолжение речи",
        );
        source.words = vec![
            timed_word("первая", 0.2, 0.7),
            timed_word("фраза", 0.8, 1.3),
            timed_word("говорит", 1.4, 1.9),
            timed_word("да", 4.25, 4.55),
            timed_word("продолжение", 4.8, 5.5),
            timed_word("речи", 5.6, 6.0),
        ];
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert!(
            out.iter()
                .any(|u| u.speaker == "B" && u.text.contains("да")),
            "короткий ответ после паузы потерял спикера: {out:?}"
        );
    }

    #[test]
    fn ctc_word_times_override_proportional_bounds() {
        let tree = DiarIntervalTree::new(&[utt(0.0, 2.0, "A", "a"), utt(2.0, 4.0, "B", "b")]);
        let mut source = utt(0.0, 4.0, "unknown", "раз два три");
        source.words = vec![
            timed_word("раз", 0.2, 1.8),
            timed_word("два", 2.2, 2.8),
            timed_word("три", 3.0, 3.5),
        ];
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "раз");
        assert_eq!(out[0].speaker, "A");
        assert_eq!(out[1].text, "два три");
        assert_eq!(out[1].speaker, "B");
        assert!((out[0].end - 1.8).abs() < 1e-6);
        assert!((out[1].start - 2.2).abs() < 1e-6);
    }

    #[test]
    fn single_word_uses_dominant_speaker() {
        let tree = DiarIntervalTree::new(&[utt(0.0, 2.0, "A", "a")]);
        let source = utt(0.0, 2.0, "", "yes");
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].speaker, "A");
        assert_eq!(out[0].text, "yes");
        assert!(out[0].diarized);
    }

    #[test]
    fn uneven_words_get_proportional_time_not_equal_steps() {
        let tree = DiarIntervalTree::new(&[utt(0.0, 1.5, "A", "a"), utt(1.5, 4.0, "B", "b")]);
        let source = utt(0.0, 4.0, "unknown", "а длинноеслово");
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].speaker, "A");
        assert_eq!(out[1].speaker, "B");
        assert!(out[0].end < 1.0, "короткое слово не должно занять 2 с");
        assert_eq!(out[1].end, source.end);
        assert_eq!(
            out.iter()
                .map(|u| u.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            source.text
        );
    }

    #[test]
    fn word_boundary_snaps_to_nearby_pause() {
        let tree = DiarIntervalTree::new(&[utt(0.0, 0.5, "A", "a"), utt(0.5, 4.0, "B", "b")]);
        let source = utt(0.0, 4.0, "unknown", "а длинноеслово");
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].speaker, "A");
        assert_eq!(out[0].text, "а");
        assert!((out[0].end - 0.5).abs() < 0.01, "{}", out[0].end);
        assert_eq!(out[1].text, "длинноеслово");
    }

    #[test]
    fn nearby_boundaries_do_not_collapse_word_to_zero() {
        let long = "x".repeat(96);
        let tree = DiarIntervalTree::new(&[utt(0.0, 0.3, "A", "a"), utt(0.3, 10.0, "B", "b")]);
        let source = utt(0.0, 10.0, "unknown", &format!("а б {long}"));
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert!(out.iter().all(|u| u.end > u.start));
        assert_eq!(
            out.iter()
                .map(|u| u.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            source.text
        );
    }

    #[test]
    fn many_words_in_short_span_keep_positive_duration() {
        let tree = DiarIntervalTree::new(&[utt(0.0, 0.03, "A", "a"), utt(0.03, 0.06, "B", "b")]);
        let source = utt(0.0, 0.06, "unknown", "а б в г д е");
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert!(out.iter().all(|u| u.end > u.start));
        assert_eq!(
            out.iter()
                .map(|u| u.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            source.text
        );
    }

    #[test]
    fn pause_snap_picks_nearest_edge_binary_search() {
        let tree = DiarIntervalTree::new(&[
            utt(0.0, 0.4, "A", "a"),
            utt(0.4, 1.0, "B", "b"),
            utt(1.0, 3.0, "C", "c"),
        ]);
        assert_eq!(tree.snap_to_pause(0.42, 0.15), Some(0.4));
        assert_eq!(tree.snap_to_pause(0.98, 0.15), Some(1.0));
        assert_eq!(tree.snap_to_pause(0.7, 0.15), None);
    }

    #[test]
    fn empty_tree_preserves_source() {
        let source = utt(0.0, 4.0, "A", "one two");
        let out = split_utterance_by_word_speakers(&source, &DiarIntervalTree::new(&[]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, source.text);
        assert_eq!(out[0].speaker, source.speaker);
    }

    #[test]
    fn partial_ctc_interpolates_missing_and_keeps_boundary_speaker() {
        let tree = DiarIntervalTree::new(&[utt(0.0, 3.0, "A", "a"), utt(3.0, 6.0, "B", "b")]);
        let mut source = utt(0.0, 6.0, "unknown", "первое второе третье четвертое");
        source.words = vec![
            timed_word("первое", 0.5, 1.2),
            WordConf {
                w: "второе".to_string(),
                conf: 0.0,
                needs_review: true,
                start_sec: 0.0,
                end_sec: 0.0,
            },
            timed_word("третье", 2.2, 2.8),
            timed_word("четвертое", 3.2, 3.8),
        ];
        let out = split_utterance_by_word_speakers(&source, &tree);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "первое второе третье");
        assert_eq!(out[0].speaker, "A");
        assert_eq!(out[1].text, "четвертое");
        assert_eq!(out[1].speaker, "B");
    }
}
