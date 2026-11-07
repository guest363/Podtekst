use std::collections::BTreeSet;

use crate::text::Transcript;

pub const TARGET_CHARS: usize = 5_500;
pub const MIN_CHARS: usize = 2_000;
pub const MAX_CHARS: usize = 7_500;
pub const MAX_CHUNKS: usize = 24;
pub const MAX_OVERLAP_CHARS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// Полуинтервал `[start, end)` по индексам реплик.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkPlan {
    pub chunks: Vec<Chunk>,
    pub truncated: bool,
    pub coverage: f64,
}

impl ChunkPlan {
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

pub fn plan(transcript: &Transcript, max_chunks: usize) -> ChunkPlan {
    let texts: Vec<&str> = transcript
        .utterances
        .iter()
        .map(|u| u.text.trim())
        .collect();
    let chars: Vec<usize> = texts.iter().map(|t| t.chars().count()).collect();
    let total: usize = chars.iter().sum();
    if total == 0 {
        return ChunkPlan {
            chunks: Vec::new(),
            truncated: false,
            coverage: 0.0,
        };
    }

    let mut chunks = greedy(&chars);
    merge_small(&mut chunks, &chars);
    if chunks.len() > max_chunks {
        merge_to_limit(&mut chunks, &chars, max_chunks);
    }
    let truncated = chunks.len() > max_chunks;
    if truncated {
        chunks.truncate(max_chunks);
    }
    let chunks = with_overlap(&chunks, &chars);
    let coverage = coverage(&chunks, &chars, total);
    ChunkPlan {
        chunks,
        truncated,
        coverage,
    }
}

fn greedy(chars: &[usize]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut current = 0usize;
    for (i, len) in chars.iter().enumerate() {
        current += *len;
        if current >= TARGET_CHARS {
            chunks.push(Chunk { start, end: i + 1 });
            start = i + 1;
            current = 0;
        }
    }
    if start < chars.len() {
        chunks.push(Chunk {
            start,
            end: chars.len(),
        });
    }
    chunks
}

fn chunk_chars(chunk: Chunk, chars: &[usize]) -> usize {
    chars[chunk.start..chunk.end].iter().sum()
}

fn merge_small(chunks: &mut Vec<Chunk>, chars: &[usize]) {
    let mut i = 0;
    while i < chunks.len() && chunks.len() > 1 {
        if chunk_chars(chunks[i], chars) < MIN_CHARS {
            if i + 1 < chunks.len()
                && chunk_chars(chunks[i], chars) + chunk_chars(chunks[i + 1], chars) <= MAX_CHARS
            {
                chunks[i].end = chunks[i + 1].end;
                chunks.remove(i + 1);
                continue;
            }
            if i > 0
                && chunk_chars(chunks[i - 1], chars) + chunk_chars(chunks[i], chars) <= MAX_CHARS
            {
                chunks[i - 1].end = chunks[i].end;
                chunks.remove(i);
                i = i.saturating_sub(1);
                continue;
            }
        }
        i += 1;
    }
}

fn merge_to_limit(chunks: &mut Vec<Chunk>, chars: &[usize], limit: usize) {
    while chunks.len() > limit {
        let mut best: Option<(usize, usize)> = None;
        for i in 0..chunks.len() - 1 {
            let combined = chunk_chars(chunks[i], chars) + chunk_chars(chunks[i + 1], chars);
            if combined <= MAX_CHARS && best.is_none_or(|(_, size)| combined < size) {
                best = Some((i, combined));
            }
        }
        let Some((i, _)) = best else {
            break;
        };
        chunks[i].end = chunks[i + 1].end;
        chunks.remove(i + 1);
    }
}

fn coverage(chunks: &[Chunk], chars: &[usize], total: usize) -> f64 {
    let unique: BTreeSet<usize> = chunks
        .iter()
        .flat_map(|c| c.start..c.end)
        .filter(|i| *i < chars.len())
        .collect();
    let covered: usize = unique.iter().map(|i| chars[*i]).sum();
    if total == 0 {
        0.0
    } else {
        covered as f64 / total as f64
    }
}

pub fn with_overlap(chunks: &[Chunk], chars: &[usize]) -> Vec<Chunk> {
    let mut out = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 {
            out.push(*chunk);
            continue;
        }
        let prev = chunks[i - 1];
        let last = prev.end - 1;
        if chars.get(last).is_some_and(|len| *len <= MAX_OVERLAP_CHARS) {
            out.push(Chunk {
                start: last,
                end: chunk.end,
            });
        } else {
            out.push(*chunk);
        }
    }
    out
}

pub fn map_bullets(detail: crate::preset::Detail) -> usize {
    detail.map_bullets()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Utterance;

    fn transcript(lengths: &[usize]) -> Transcript {
        Transcript {
            utterances: lengths
                .iter()
                .enumerate()
                .map(|(i, len)| Utterance {
                    start: i as f64,
                    end: i as f64 + 1.0,
                    speaker: "A".into(),
                    text: "я".repeat(*len),
                })
                .collect(),
        }
    }

    #[test]
    fn short_source_is_one_chunk_with_full_coverage() {
        let plan = plan(&transcript(&[100, 200, 300]), MAX_CHUNKS);
        assert_eq!(plan.chunks.len(), 1);
        assert!(!plan.truncated);
        assert_eq!(plan.coverage, 1.0);
    }

    #[test]
    fn greedy_respects_target_and_max() {
        let plan = plan(&transcript(&[1_000; 20]), MAX_CHUNKS);
        assert!(plan.chunks.len() > 1);
        for chunk in &plan.chunks {
            let size = chunk_chars(*chunk, &[1_000usize; 20]);
            assert!(size <= MAX_CHARS || chunk.end - chunk.start == 1);
        }
        assert_eq!(plan.coverage, 1.0);
    }

    #[test]
    fn more_than_limit_is_truncated_with_exact_coverage() {
        let plan = plan(&transcript(&[6_000; 30]), MAX_CHUNKS);
        assert_eq!(plan.chunks.len(), MAX_CHUNKS);
        assert!(plan.truncated);
        assert!(plan.coverage > 0.0 && plan.coverage < 1.0);
    }

    #[test]
    fn overlap_uses_only_short_previous_utterance() {
        let chunks = [Chunk { start: 0, end: 3 }, Chunk { start: 3, end: 5 }];
        let chars = [500, 500, 200, 500, 500];
        let overlapped = with_overlap(&chunks, &chars);
        assert_eq!(overlapped[1].start, 2);
        let chars_long = [500, 500, 500, 500, 500];
        assert_eq!(with_overlap(&chunks, &chars_long)[1].start, 3);
    }

    #[test]
    fn empty_transcript_has_no_chunks() {
        let plan = plan(&Transcript::default(), MAX_CHUNKS);
        assert!(plan.is_empty());
        assert_eq!(plan.coverage, 0.0);
    }
}
