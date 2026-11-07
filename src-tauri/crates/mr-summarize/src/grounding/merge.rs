use std::collections::HashSet;

use super::Scored;
use crate::artifact::SummaryItem;
use crate::parse::ParsedSection;
use crate::preset::{Detail, Preset};
use crate::text::{self, Transcript};

pub(super) fn merge_fallback(
    sections: &mut [Vec<Scored>],
    specs: &[ParsedSection],
    transcript: &Transcript,
    preset: &Preset,
) -> usize {
    let mut added = 0usize;
    for candidate in crate::extract::candidates(transcript, preset) {
        let Some(index) = specs.iter().position(|s| s.id == candidate.section_id) else {
            continue;
        };
        if sections[index]
            .iter()
            .any(|existing| same_task_or_evidence(&existing.item, &candidate.item))
        {
            continue;
        }
        sections[index].push(Scored {
            item: candidate.item,
            ev: 1.0,
        });
        added += 1;
    }
    added
}

fn same_task_or_evidence(model: &SummaryItem, fallback: &SummaryItem) -> bool {
    if model.refs.iter().any(|a| {
        fallback
            .refs
            .iter()
            .any(|b| a.utterance_index == b.utterance_index)
    }) {
        return true;
    }
    let a = text::stem_set(&model.text);
    let b = text::stem_set(&fallback.text);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let intersection = a.intersection(&b).count();
    let union = a.union(&b).count();
    let jaccard = intersection as f64 / union.max(1) as f64;
    let containment = intersection as f64 / a.len().min(b.len()).max(1) as f64;
    jaccard >= 0.6 || containment >= 0.8
}

pub(super) fn dedupe(items: &mut Vec<Scored>) {
    let mut kept: Vec<Scored> = Vec::new();
    for candidate in items.drain(..) {
        let tokens = text::stem_set(&candidate.item.text);
        let duplicate = kept.iter_mut().find(|existing| {
            let other = text::stem_set(&existing.item.text);
            jaccard(&tokens, &other) >= 0.8
        });
        match duplicate {
            Some(existing) if candidate.ev > existing.ev => *existing = candidate,
            Some(_) => {}
            None => kept.push(candidate),
        }
    }
    *items = kept;
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        a.intersection(b).count() as f64 / union as f64
    }
}

pub(super) fn cap(sections: &mut [Vec<Scored>], detail: Detail) {
    let max = detail.max_items();
    let total: usize = sections.iter().map(Vec::len).sum();
    if total <= max {
        return;
    }
    let mut selected: HashSet<(usize, usize)> = HashSet::new();
    for (i, items) in sections.iter().enumerate() {
        if !items.is_empty() && selected.len() < max {
            selected.insert((i, 0));
        }
    }
    let mut rest: Vec<(usize, usize, f64)> = sections
        .iter()
        .enumerate()
        .flat_map(|(i, items)| {
            items
                .iter()
                .enumerate()
                .filter(|(j, _)| *j > 0)
                .map(move |(j, scored)| (i, j, scored.ev))
        })
        .collect();
    rest.sort_by(|a, b| b.2.total_cmp(&a.2));
    for (i, j, _) in rest {
        if selected.len() >= max {
            break;
        }
        selected.insert((i, j));
    }
    for (i, items) in sections.iter_mut().enumerate() {
        let mut index = 0;
        items.retain(|_| {
            let keep = selected.contains(&(i, index));
            index += 1;
            keep
        });
    }
}
