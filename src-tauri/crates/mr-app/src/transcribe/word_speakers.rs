//! Пословная атрибуция спикеров по размеченным фрагментам диаризации.

use std::collections::BTreeMap;

use super::Utterance;

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
        Self { segments, root }
    }

    /// Спикер с наибольшей точной площадью пересечения и взвешенный силуэт.
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
}

/// Делит ASR-реплику строго на стыках слов с разными доминирующими спикерами.
pub(crate) fn split_utterance_by_word_speakers(
    utterance: &Utterance,
    tree: &DiarIntervalTree,
) -> Vec<Utterance> {
    let words: Vec<&str> = utterance.text.split_whitespace().collect();
    if words.is_empty() || utterance.end <= utterance.start || tree.root.is_none() {
        return vec![utterance.clone()];
    }
    // shortcut: ASR пока не возвращает word timestamps, интерполяция; заменить при появлении alignment.
    let word_count = words.len();
    let step = (utterance.end - utterance.start) / word_count as f64;
    let mut assigned = Vec::with_capacity(word_count);
    for (i, word) in words.into_iter().enumerate() {
        let start = utterance.start + i as f64 * step;
        let end = if i + 1 == word_count {
            utterance.end
        } else {
            start + step
        };
        let (speaker, silhouette) = tree
            .assign_dominant_speaker(start, end)
            .unwrap_or_else(|| (utterance.speaker.clone(), utterance.silhouette));
        assigned.push((word, start, end, speaker, silhouette));
    }

    let mut out: Vec<Utterance> = Vec::new();
    for (word, start, end, speaker, silhouette) in assigned {
        if let Some(last) = out.last_mut() {
            if last.speaker == speaker {
                let old_dur = (last.end - last.start).max(0.0);
                let new_dur = (end - start).max(0.0);
                last.silhouette = (last.silhouette * old_dur + silhouette * new_dur)
                    / (old_dur + new_dur).max(f64::EPSILON);
                last.end = end;
                last.text.push(' ');
                last.text.push_str(word);
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
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utt(start: f64, end: f64, speaker: &str, text: &str) -> Utterance {
        Utterance {
            start,
            end,
            speaker: speaker.to_string(),
            channel: None,
            text: text.to_string(),
            diarized: true,
            silhouette: 0.5,
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
    fn empty_tree_preserves_source() {
        let source = utt(0.0, 4.0, "A", "one two");
        let out = split_utterance_by_word_speakers(&source, &DiarIntervalTree::new(&[]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, source.text);
        assert_eq!(out[0].speaker, source.speaker);
    }
}
