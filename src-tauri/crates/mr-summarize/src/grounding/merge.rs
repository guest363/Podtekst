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
        if let Some(position) = sections[index]
            .iter()
            .position(|existing| same_task_or_evidence(&existing.item, &candidate.item))
        {
            if metadata_score(&candidate.item) > metadata_score(&sections[index][position].item) {
                sections[index][position] = Scored {
                    item: candidate.item,
                    ev: 1.0,
                };
                added += 1;
            }
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
    let task_like = model.task.is_some() || fallback.task.is_some();
    if task_like {
        let model_key = crate::extract::task_object_key(&model.text);
        let fallback_key = crate::extract::task_object_key(&fallback.text);
        if model_key.is_some() && model_key == fallback_key {
            return true;
        }
    }
    if task_like {
        intersection >= 2 && (jaccard >= 0.3 || containment >= 0.5)
    } else {
        jaccard >= 0.6 || containment >= 0.8
    }
}

fn metadata_score(item: &SummaryItem) -> u8 {
    let Some(task) = item.task.as_ref() else {
        return 0;
    };
    u8::from(task.assignee.is_some()) + u8::from(task.deadline.is_some())
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

pub(super) fn cap(sections: &mut [Vec<Scored>], specs: &[ParsedSection], detail: Detail) {
    let max = detail.max_items();
    if let Some(brief) = specs.iter().position(|s| s.id == "brief") {
        // Модель любит 10+ пунктов «Кратко»; оставляем только главное,
        // чтобы шум не вытеснял задачи/решения.
        sections[brief].truncate(4);
    }
    let mut selected: HashSet<(usize, usize)> = HashSet::new();
    let task_ids = &["tasks", "next_steps", "todo"][..];
    // 1. Все задачи — до второго решения/доп. brief.
    for (section_index, spec) in specs.iter().enumerate() {
        if !task_ids.contains(&spec.id) {
            continue;
        }
        for item_index in 0..sections[section_index].len() {
            if selected.len() >= max {
                break;
            }
            selected.insert((section_index, item_index));
        }
        if selected.len() >= max {
            break;
        }
    }
    // 2. Первое решение, первый вопрос, первый brief, первые пункты остальных
    // секций — чтобы ни одна непустая секция не исчезла.
    for ids in [
        &["decisions"][..],
        &["questions"][..],
        &["brief"][..],
        &[][..],
    ] {
        let wildcard = ids.is_empty();
        for (section_index, spec) in specs.iter().enumerate() {
            if selected.len() >= max {
                break;
            }
            let target = if wildcard {
                !task_ids.contains(&spec.id)
                    && !matches!(spec.id, "decisions" | "questions" | "brief")
            } else {
                ids.contains(&spec.id)
            };
            if target && !sections[section_index].is_empty() {
                selected.insert((section_index, 0));
            }
        }
    }
    // 3. Второе и следующие решения.
    for (section_index, spec) in specs.iter().enumerate() {
        if spec.id != "decisions" {
            continue;
        }
        for item_index in 1..sections[section_index].len() {
            if selected.len() >= max {
                break;
            }
            selected.insert((section_index, item_index));
        }
    }
    // 4. Остальные вопросы и brief (brief уже <= 4).
    for ids in [&["questions"][..], &["brief"][..]] {
        for (section_index, spec) in specs.iter().enumerate() {
            if !ids.contains(&spec.id) {
                continue;
            }
            for item_index in 1..sections[section_index].len() {
                if selected.len() >= max {
                    break;
                }
                selected.insert((section_index, item_index));
            }
        }
    }
    if selected.len() < max {
        let mut rest: Vec<(usize, usize, f64)> = sections
            .iter()
            .enumerate()
            .flat_map(|(i, items)| {
                items
                    .iter()
                    .enumerate()
                    .map(move |(j, scored)| (i, j, scored.ev))
            })
            .filter(|(i, j, _)| !selected.contains(&(*i, *j)))
            .collect();
        rest.sort_by(|a, b| b.2.total_cmp(&a.2));
        for (i, j, _) in rest {
            if selected.len() >= max {
                break;
            }
            selected.insert((i, j));
        }
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
