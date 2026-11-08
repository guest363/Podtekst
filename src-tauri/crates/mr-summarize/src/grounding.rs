use crate::artifact::{SummaryItem, SummarySection, TaskMeta, Warning};
use crate::deadline;
use crate::parse::ParsedSummary;
use crate::preset::{self, Detail, Preset, SectionRole};
use crate::text::{self, Language, Transcript, Utterance};

mod merge;
mod rules;
use rules::{decision_cue, hard_facts_supported};

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
    language: Language,
) -> GroundedSummary {
    let mut warnings = parsed.warnings.clone();
    let mut dropped = 0usize;
    let mut sections: Vec<Vec<Scored>> = parsed.sections.iter().map(|_| Vec::new()).collect();
    let brief_index = parsed.sections.iter().position(|s| s.id == "brief");
    let known = crate::extract::speaker_names(&transcript.utterances);
    let decision_cue = has_decision_cue(transcript);

    for (section_index, section) in parsed.sections.iter().enumerate() {
        for item in &section.items {
            if !hard_facts_supported(&item.text, &transcript.utterances) {
                dropped += 1;
                continue;
            }
            let mut output = SummaryItem {
                text: item.text.clone(),
                role: section.role,
                task: item.task.clone(),
            };
            if section.role == SectionRole::Task {
                let task =
                    reconcile_task(&mut output, &transcript.utterances, &known, &mut warnings);
                if !task {
                    dropped += 1;
                    continue;
                }
            }
            if section.role == SectionRole::Decision && !decision_cue {
                push_once(&mut warnings, Warning::DecisionWithoutCue);
                if let Some(brief) = brief_index {
                    output.role = SectionRole::Plain;
                    sections[brief].push(Scored {
                        item: output,
                        ev: 1.0,
                    });
                }
                continue;
            }
            sections[section_index].push(Scored {
                item: output,
                ev: 1.0,
            });
        }
    }

    for group in &mut sections {
        merge::dedupe(group);
    }

    if let Some(brief) = brief_index {
        let needed = 2usize.saturating_sub(sections[brief].len());
        if needed > 0 {
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
            let mut added = 0usize;
            for (i, j, _) in candidates {
                if added >= needed {
                    break;
                }
                let candidate = &sections[i][j];
                if brief_duplicate(&sections[brief], &candidate.item) {
                    continue;
                }
                let mut item = candidate.item.clone();
                item.role = SectionRole::Plain;
                item.task = None;
                let ev = candidate.ev;
                sections[brief].push(Scored { item, ev });
                added += 1;
            }
            if added > 0 {
                push_once(&mut warnings, Warning::BriefDerived);
            }
        }
    }

    // Fallback — последний резерв: его дословные реплики не выдаём за «Кратко».
    let fallback_added = merge::merge_fallback(&mut sections, &parsed.sections, transcript, preset);
    if fallback_added > 0 {
        push_once(&mut warnings, Warning::FallbackExtracted);
    }

    merge::cap(&mut sections, &parsed.sections, detail);
    if dropped > 0 {
        push_once(&mut warnings, Warning::ItemsDroppedUnsupported);
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
                    .map(|s| {
                        if language == Language::En {
                            s.title_en
                        } else {
                            s.title_ru
                        }
                    })
                    .unwrap_or(spec.title.as_str())
                    .to_owned(),
                role: spec.role,
                items: items.into_iter().map(|s| s.item).collect(),
            })
        })
        .collect();

    GroundedSummary {
        sections: ordered,
        warnings,
        items_dropped: dropped,
        soft_claims_dropped: 0,
        parser_recovered: parsed.parser_recovered,
    }
}

fn brief_duplicate(existing: &[Scored], candidate: &SummaryItem) -> bool {
    existing.iter().any(|scored| {
        let item = &scored.item;
        let a = text::stem_set(&item.text);
        let b = text::stem_set(&candidate.text);
        if a.is_empty() || b.is_empty() {
            return false;
        }
        let intersection = a.intersection(&b).count();
        intersection as f64 / a.len().min(b.len()).max(1) as f64 >= 0.8
    })
}

fn adjacent(indices: &[usize], len: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for index in indices {
        for candidate in [index.saturating_sub(1), *index, *index + 1] {
            if candidate < len && !out.contains(&candidate) {
                out.push(candidate);
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

fn best_utterance_index(item: &str, utterances: &[Utterance]) -> Option<usize> {
    let item_tokens = text::stem_set(item);
    if item_tokens.is_empty() {
        return None;
    }
    utterances
        .iter()
        .enumerate()
        .map(|(index, utterance)| {
            (
                index,
                item_tokens
                    .intersection(&text::stem_set(&utterance.text))
                    .count(),
            )
        })
        .filter(|(_, overlap)| *overlap > 0)
        .max_by(|(a_index, a), (b_index, b)| a.cmp(b).then_with(|| b_index.cmp(a_index)))
        .map(|(index, _)| index)
}

fn reconcile_task(
    output: &mut SummaryItem,
    utterances: &[Utterance],
    known: &[String],
    warnings: &mut Vec<Warning>,
) -> bool {
    let best_index = best_utterance_index(&output.text, utterances);
    // Ворота задачи смотрят только реплику-источник, без соседей: иначе
    // соседняя задача делает задачей соседнее решение.
    let best_support = best_index
        .map(|index| support_text(utterances, &[index]))
        .unwrap_or_default();
    let normalized_best = text::normalize(&best_support);
    let primary = best_index
        .map(|index| adjacent(&[index], utterances.len()))
        .unwrap_or_default();
    let support = support_text(utterances, &primary);
    let normalized_support = text::normalize(&support);
    let normalized_text = text::normalize(&output.text);
    if crate::cues::task_rejected(&output.text) || crate::cues::task_rejected(&best_support) {
        return false;
    }
    let kind = crate::extract::task_kind(&output.text, &normalized_text, known)
        .or_else(|| crate::extract::task_kind(&best_support, &normalized_best, known));
    let explicit = kind.is_some()
        || (crate::extract::explicit_action(&output.text)
            && (crate::extract::explicit_action(&best_support)
                || crate::extract::explicit_action(&support)));
    if !explicit {
        return false;
    }
    if output.task.is_none() {
        output.task = Some(TaskMeta {
            assignee: None,
            deadline: None,
            deadline_source: crate::artifact::DeadlineSource::Model,
        });
    }
    let task = output.task.as_mut().expect("task was just set");
    // Дедлайн — только из лучшей реплики: соседняя реплика часто про другую
    // задачу, а «на пятницу» из неё — ложный срок (наблюдение E2E).
    let found = best_index.and_then(|index| deadline::find(utterances, &[index]));
    let outcome = deadline::reconcile(task.deadline.as_deref(), found.as_ref());
    task.deadline = outcome.deadline;
    task.deadline_source = outcome.source;
    if let Some(warning) = outcome.warning {
        push_once(warnings, warning);
    }

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
    let had_assignee = task.assignee.is_some();
    if let Some(assignee) = task.assignee.clone() {
        let normalized = text::normalize(&assignee);
        let speaker_match = speaker
            .as_deref()
            .map(|s| text::normalize(s) == normalized)
            .unwrap_or(false);
        if !name_mentioned(&normalized_support, &normalized)
            && !name_mentioned(&normalized_text, &normalized)
            && !speaker_match
        {
            task.assignee = None;
            push_once(warnings, Warning::AssigneeNotStated);
        }
    }
    if task.assignee.is_none() && !had_assignee {
        task.assignee = crate::extract::assignee_for(
            &output.text,
            Some(&support),
            speaker.as_deref(),
            known,
            kind.unwrap_or(crate::extract::TaskKind::Explicit),
        );
    }
    true
}

/// Имя может стоять в падеже («Анне»): сравниваем начало слова, как в
/// `extract::name_key`, а не только точное вхождение.
fn name_mentioned(text: &str, name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    let take = if chars.len() <= 4 { 3 } else { 5 };
    let key: String = chars.into_iter().take(take).collect();
    !key.is_empty() && text.split_whitespace().any(|token| token.starts_with(&key))
}

fn push_once(warnings: &mut Vec<Warning>, warning: Warning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

#[cfg(test)]
mod tests;
