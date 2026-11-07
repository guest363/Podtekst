use crate::artifact::{SummaryItem, SummaryRef, SummarySection, TaskMeta, Warning};
use crate::deadline;
use crate::parse::{ParsedItem, ParsedSummary};
use crate::preset::{self, Detail, Preset, SectionRole};
use crate::text::{self, Transcript, Utterance};
use std::collections::HashSet;

mod merge;
mod rules;
use rules::{decision_cue, hard_facts_supported, soft_claim_unsupported};

#[derive(Debug, Clone, PartialEq)]
pub struct GroundedSummary {
    pub sections: Vec<SummarySection>,
    pub warnings: Vec<Warning>,
    pub items_dropped: usize,
    pub soft_claims_dropped: usize,
    pub parser_recovered: usize,
}

struct Scored {
    item: SummaryItem,
    ev: f64,
}

/// Есть ли в источнике явный decision-cue: нужен для `decision_missed`
/// retry/flag без дублирования таблицы cue в mr-app.
pub fn has_decision_cue(transcript: &Transcript) -> bool {
    decision_cue(&text::normalize(&transcript.full_text()))
}

pub fn ground(
    parsed: &ParsedSummary,
    transcript: &Transcript,
    preset: &Preset,
    detail: Detail,
) -> GroundedSummary {
    let mut warnings = parsed.warnings.clone();
    let mut dropped = 0usize;
    let mut soft = 0usize;
    let mut sections: Vec<Vec<Scored>> = parsed.sections.iter().map(|_| Vec::new()).collect();
    let brief_index = parsed.sections.iter().position(|s| s.id == "brief");

    for (section_index, section) in parsed.sections.iter().enumerate() {
        for item in &section.items {
            let Some((refs, evidence)) = support(item, &transcript.utterances) else {
                dropped += 1;
                continue;
            };
            if !hard_facts_supported(&item.text, &transcript.utterances, &refs) {
                dropped += 1;
                continue;
            }
            let support_indices = adjacent(&refs, transcript.utterances.len());
            let support_text = support_text(&transcript.utterances, &support_indices);
            if soft_claim_unsupported(
                item,
                &support_text,
                preset,
                &transcript.utterances,
                &support_indices,
            ) {
                soft += 1;
                continue;
            }
            let quote = validated_quote(
                item.quote.as_deref(),
                &transcript.utterances,
                &support_indices_refs(&refs),
            );
            let mut output = SummaryItem {
                text: item.text.clone(),
                role: section.role,
                task: item.task.clone(),
                refs,
                quote,
            };
            if section.role == SectionRole::Task {
                let task = reconcile_task(&mut output, &transcript.utterances, &mut warnings);
                if !task {
                    soft += 1;
                    continue;
                }
            }
            if section.role == SectionRole::Decision && !decision_cue(&support_text) {
                push_once(&mut warnings, Warning::DecisionWithoutCue);
                if let Some(brief) = brief_index {
                    output.role = SectionRole::Plain;
                    sections[brief].push(Scored {
                        item: output,
                        ev: evidence,
                    });
                }
                continue;
            }
            sections[section_index].push(Scored {
                item: output,
                ev: evidence,
            });
        }
    }

    for group in &mut sections {
        merge::dedupe(group);
    }

    let fallback_added = merge::merge_fallback(&mut sections, &parsed.sections, transcript, preset);
    if fallback_added > 0 {
        push_once(&mut warnings, Warning::FallbackExtracted);
    }

    if let Some(brief) = brief_index {
        if sections[brief].is_empty() {
            let mut candidates: Vec<(usize, usize, f64)> = sections
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != brief)
                .filter(|(i, _)| {
                    matches!(
                        parsed.sections[*i].id,
                        "decisions" | "tasks" | "next_steps" | "todo"
                    )
                })
                .flat_map(|(i, items)| {
                    items
                        .iter()
                        .enumerate()
                        .map(move |(j, scored)| (i, j, scored.ev))
                })
                .collect();
            candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
            for (i, j, _) in candidates.into_iter().take(3) {
                let mut item = sections[i][j].item.clone();
                item.role = SectionRole::Plain;
                item.task = None;
                let ev = sections[i][j].ev;
                sections[brief].push(Scored { item, ev });
            }
            if !sections[brief].is_empty() {
                push_once(&mut warnings, Warning::BriefDerived);
            }
        }
    }

    merge::cap(&mut sections, detail);
    if dropped > 0 {
        push_once(&mut warnings, Warning::ItemsDroppedUnsupported);
    }
    if soft > 0 {
        push_once(&mut warnings, Warning::SoftClaimDropped);
    }
    if parsed.parser_recovered > 0 {
        push_once(&mut warnings, Warning::ParserRecovered);
    }

    let ordered: Vec<SummarySection> = parsed
        .sections
        .iter()
        .zip(sections)
        .filter_map(|(spec, items)| {
            if items.is_empty() {
                return None;
            }
            Some(SummarySection {
                id: spec.id.to_owned(),
                title: preset::section(preset, spec.id)
                    .map(|s| s.title_ru.to_owned())
                    .unwrap_or_else(|| spec.title.clone()),
                role: spec.role,
                items: items.into_iter().map(|s| s.item).collect(),
            })
        })
        .collect();

    GroundedSummary {
        sections: ordered,
        warnings,
        items_dropped: dropped,
        soft_claims_dropped: soft,
        parser_recovered: parsed.parser_recovered,
    }
}

fn support(item: &ParsedItem, utterances: &[Utterance]) -> Option<(Vec<SummaryRef>, f64)> {
    let item_tokens: HashSet<String> = text::stem_set(&item.text);
    if item_tokens.is_empty() {
        return None;
    }
    let mut matches: Vec<(usize, usize, f64)> = Vec::new();
    for (index, utterance) in utterances.iter().enumerate() {
        let utterance_tokens = text::stem_set(&utterance.text);
        let intersection = item_tokens.intersection(&utterance_tokens).count();
        if intersection < 2 {
            continue;
        }
        let ev = intersection as f64 / item_tokens.len().max(1) as f64;
        if ev >= 0.25 {
            matches.push((index, intersection, ev));
        }
    }
    matches.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    let best = matches.first()?.2;
    let refs = matches
        .into_iter()
        .take(3)
        .map(|(index, _, _)| SummaryRef {
            utterance_index: index,
            start: utterances[index].start,
        })
        .collect();
    Some((refs, best))
}

fn adjacent(refs: &[SummaryRef], len: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for r in refs {
        for index in [
            r.utterance_index.saturating_sub(1),
            r.utterance_index,
            r.utterance_index + 1,
        ] {
            if index < len && !out.contains(&index) {
                out.push(index);
            }
        }
    }
    out
}

fn support_text(utterances: &[Utterance], indices: &[usize]) -> String {
    indices
        .iter()
        .filter_map(|i| utterances.get(*i))
        .map(|u| text::normalize(&u.text))
        .collect::<Vec<_>>()
        .join(" ")
}

fn support_indices_refs(refs: &[SummaryRef]) -> Vec<usize> {
    refs.iter().map(|r| r.utterance_index).collect()
}

fn validated_quote(
    quote: Option<&str>,
    utterances: &[Utterance],
    indices: &[usize],
) -> Option<String> {
    let quote = quote?;
    let normalized = text::normalize(quote);
    if normalized.is_empty() {
        return None;
    }
    indices
        .iter()
        .filter_map(|i| utterances.get(*i))
        .any(|u| {
            let utterance = text::normalize(&u.text);
            format!(" {utterance} ").contains(&format!(" {normalized} "))
        })
        .then(|| quote.to_owned())
}

fn reconcile_task(
    output: &mut SummaryItem,
    utterances: &[Utterance],
    warnings: &mut Vec<Warning>,
) -> bool {
    let primary = output
        .refs
        .first()
        .map(|r| adjacent(std::slice::from_ref(r), utterances.len()))
        .unwrap_or_default();
    let support = support_text(utterances, &primary);
    if !crate::cues::task_cue(&support) {
        return false;
    }
    if output.task.is_none() {
        output.task = Some(TaskMeta {
            assignee: None,
            deadline: None,
            deadline_ref: None,
            deadline_source: crate::artifact::DeadlineSource::Model,
        });
    }
    let task = output.task.as_mut().expect("task was just set");
    // Дедлайн — только из лучшей реплики: соседняя реплика часто про другую
    // задачу, а «на пятницу» из неё — ложный срок (наблюдение E2E).
    let best_index = output.refs.first().map(|r| r.utterance_index);
    let found = best_index.and_then(|index| deadline::find(utterances, &[index]));
    let outcome = deadline::reconcile(task.deadline.as_deref(), found.as_ref());
    task.deadline = outcome.deadline;
    task.deadline_ref = outcome.reference;
    task.deadline_source = outcome.source;
    if let Some(warning) = outcome.warning {
        push_once(warnings, warning);
    }

    let normalized_support = text::normalize(&support);
    let normalized_text = text::normalize(&output.text);
    let speaker = best_index
        .and_then(|index| utterances.get(index))
        .map(|u| u.speaker.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            primary
                .iter()
                .filter_map(|index| utterances.get(*index))
                .map(|u| u.speaker.trim())
                .find(|s| !s.is_empty())
                .map(str::to_owned)
        });
    if let Some(assignee) = task.assignee.clone() {
        let normalized = text::normalize(&assignee);
        let speaker_match = speaker
            .as_deref()
            .map(|s| text::normalize(s) == normalized)
            .unwrap_or(false);
        if !normalized_support.contains(&normalized)
            && !normalized_text.contains(&normalized)
            && !speaker_match
        {
            task.assignee = None;
            push_once(warnings, Warning::AssigneeNotStated);
        }
    }
    if task.assignee.is_none() {
        task.assignee =
            crate::extract::assignee_for(&output.text, Some(&support), speaker.as_deref());
    }
    true
}

fn push_once(warnings: &mut Vec<Warning>, warning: Warning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

#[cfg(test)]
mod tests;
