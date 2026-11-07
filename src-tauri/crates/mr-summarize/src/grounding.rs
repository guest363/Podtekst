use crate::artifact::{SummaryItem, SummaryRef, SummarySection, TaskMeta, Warning};
use crate::deadline;
use crate::parse::{ParsedItem, ParsedSummary};
use crate::preset::{self, Detail, Preset, SectionRole};
use crate::text::{self, Transcript, Utterance};
use std::collections::HashSet;

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
        dedupe(group);
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

    cap(&mut sections, detail);
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
    let speaker = primary
        .iter()
        .filter_map(|index| utterances.get(*index))
        .map(|u| u.speaker.trim())
        .find(|s| !s.is_empty())
        .map(str::to_owned);
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
        task.assignee = speaker_assignee(speaker.as_deref(), &normalized_support, &normalized_text);
        if task.assignee.is_none() {
            task.assignee = extract_assignee_name(&output.text);
        }
    }
    true
}

fn speaker_assignee(speaker: Option<&str>, support: &str, task_text: &str) -> Option<String> {
    let speaker = speaker?;
    let normalized = text::normalize(speaker);
    let placeholder = normalized.starts_with("спикер")
        || normalized.starts_with("speaker")
        || normalized == "гость"
        || normalized == "участник";
    if !placeholder && (support.contains(&normalized) || task_text.contains(&normalized)) {
        return Some(speaker.to_owned());
    }
    if crate::cues::task_cue(support)
        && (support.contains("я сделаю")
            || support.contains("я подготовлю")
            || support.contains("я проверю")
            || support.contains("я обновлю")
            || support.contains("я доделаю")
            || support.contains("беру")
            || support.contains("отвечаю"))
    {
        return Some(speaker.to_owned());
    }
    None
}

/// «Имя + verb» из текста задачи без `Кто:`. Имя — заглавное слово, рядом с
/// которым стоит глагол-обязательство; даты/дни/служебные метки не имена.
fn extract_assignee_name(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        let mut chars = clean.chars();
        let Some(first) = chars.next() else {
            continue;
        };
        if !first.is_uppercase() || !chars.any(char::is_lowercase) {
            continue;
        }
        let normalized = text::normalize(clean);
        if is_not_name(&normalized) {
            continue;
        }
        let prev_verb = index
            .checked_sub(1)
            .and_then(|i| words.get(i))
            .map(|w| crate::cues::action_verb(w))
            .unwrap_or(false);
        let next_verb = words
            .iter()
            .skip(index + 1)
            .take(2)
            .any(|w| crate::cues::action_verb(w));
        if prev_verb || next_verb {
            return Some(clean.to_owned());
        }
    }
    None
}

fn is_not_name(normalized: &str) -> bool {
    matches!(
        normalized,
        "срок"
            | "кто"
            | "что"
            | "задача"
            | "решение"
            | "отчет"
            | "встреча"
            | "план"
            | "документ"
            | "документация"
            | "проект"
            | "бюджет"
            | "вопрос"
            | "итог"
            | "текст"
            | "письмо"
            | "звонок"
            | "тест"
            | "релиз"
            | "презентация"
            | "макет"
            | "баг"
            | "тикет"
            | "сегодня"
            | "завтра"
            | "послезавтра"
            | "понедельник"
            | "вторник"
            | "среда"
            | "четверг"
            | "пятница"
            | "суббота"
            | "воскресенье"
    ) || normalized.starts_with("январ")
        || normalized.starts_with("феврал")
        || normalized.starts_with("март")
        || normalized.starts_with("апрел")
        || normalized.starts_with("май")
        || normalized.starts_with("июн")
        || normalized.starts_with("июл")
        || normalized.starts_with("август")
        || normalized.starts_with("сентябр")
        || normalized.starts_with("октябр")
        || normalized.starts_with("ноябр")
        || normalized.starts_with("декабр")
}

fn dedupe(items: &mut Vec<Scored>) {
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

fn cap(sections: &mut [Vec<Scored>], detail: Detail) {
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

fn push_once(warnings: &mut Vec<Warning>, warning: Warning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

#[cfg(test)]
mod tests;
