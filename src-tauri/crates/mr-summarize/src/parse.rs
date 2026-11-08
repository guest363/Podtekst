use std::sync::OnceLock;

use regex::Regex;

use crate::artifact::{DeadlineSource, TaskMeta, Warning};
use crate::cues;
use crate::error::SummaryError;
use crate::preset::{Preset, SectionRole};

mod aliases;
use aliases::resolve_heading;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedItem {
    pub text: String,
    pub role: SectionRole,
    pub task: Option<TaskMeta>,
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSection {
    pub id: &'static str,
    pub title: String,
    pub role: SectionRole,
    pub items: Vec<ParsedItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSummary {
    pub sections: Vec<ParsedSection>,
    pub warnings: Vec<Warning>,
    pub parser_recovered: usize,
    pub injection_flagged: usize,
}

/// Пустой каркас всех секций шаблона: база для merge частичных map-ответов.
pub fn empty(preset: &Preset) -> ParsedSummary {
    ParsedSummary {
        sections: preset
            .sections
            .iter()
            .map(|s| ParsedSection {
                id: s.id,
                title: s.title_ru.to_owned(),
                role: s.role,
                items: Vec::new(),
            })
            .collect(),
        warnings: Vec::new(),
        parser_recovered: 0,
        injection_flagged: 0,
    }
}

pub fn parse(output: &str, preset: &Preset) -> Result<ParsedSummary, SummaryError> {
    if !has_recognized_headings(output, preset)
        && output.lines().any(|line| !line.trim().is_empty())
    {
        return parse_flat(output, preset);
    }
    let mut sections: Vec<ParsedSection> = preset
        .sections
        .iter()
        .map(|s| ParsedSection {
            id: s.id,
            title: s.title_ru.to_owned(),
            role: s.role,
            items: Vec::new(),
        })
        .collect();
    let default_index = sections.iter().position(|s| s.id == "brief").unwrap_or(0);
    let mut current = None;
    let mut last: Option<(usize, usize)> = None;
    let mut last_raw = String::new();
    let mut recovered = 0usize;
    let mut injection = 0usize;

    for raw in output.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            last = None;
            last_raw.clear();
            continue;
        }
        if let Some(heading) = heading(trimmed) {
            match resolve_heading(preset, &heading) {
                Some(id) => {
                    current = sections.iter().position(|s| s.id == id);
                }
                None => {
                    recovered += 1;
                    if current.is_none() {
                        current = Some(default_index);
                    }
                }
            }
            last = None;
            last_raw = trimmed.to_owned();
            continue;
        }
        // Bare «Кратко:» / «Открытые вопросы:» — тоже заголовок, но только
        // если он есть в каталоге; иначе это обычная строка.
        if let Some(name) = bare_heading(trimmed) {
            if let Some(id) = resolve_heading(preset, &name) {
                current = sections.iter().position(|s| s.id == id);
                last = None;
                last_raw = trimmed.to_owned();
                continue;
            }
        }

        let current = *current.get_or_insert(default_index);
        let role = sections[current].role;
        if let Some((label, value)) = label_line(trimmed) {
            let value = clean_text(value);
            if value.is_empty() {
                last_raw = trimmed.to_owned();
                continue;
            }
            if contains_injection(&value) {
                injection += 1;
                last = None;
                last_raw = trimmed.to_owned();
                continue;
            }
            match label.as_str() {
                "задача" | "что" | "task" | "what" => {
                    if role == SectionRole::Task {
                        let (text, task) = parse_task(&value);
                        push_item(&mut sections[current], text, role, task, None);
                    } else {
                        push_item(&mut sections[current], value, role, None, None);
                    }
                    last = Some((current, sections[current].items.len() - 1));
                }
                "решение" | "decision" => {
                    push_item(&mut sections[current], value, role, None, None);
                    last = Some((current, sections[current].items.len() - 1));
                }
                "срок"
                | "deadline"
                | "due"
                | "кто"
                | "ответственный"
                | "who"
                | "assignee"
                | "owner"
                    if role == SectionRole::Task =>
                {
                    if let Some((section, item)) = last {
                        apply_task_field(&mut sections[section].items[item], &label, &value);
                    }
                }
                _ => {}
            }
            last_raw = trimmed.to_owned();
            continue;
        }

        if let Some(item_text) = item_line(trimmed) {
            let cleaned = clean_text(item_text);
            if contains_injection(&cleaned) {
                injection += 1;
                last = None;
                last_raw = trimmed.to_owned();
                continue;
            }
            let (text, quote) = split_quote(&cleaned);
            let (text, task) = if role == SectionRole::Task {
                parse_task(&text)
            } else {
                (text, None)
            };
            push_item(&mut sections[current], text, role, task, quote);
            last = Some((current, sections[current].items.len() - 1));
            last_raw = trimmed.to_owned();
            continue;
        }

        let continuation = raw.len() - raw.trim_start().len() >= 2
            || last_raw.ends_with(',')
            || last_raw.ends_with(';')
            || last_raw.ends_with(':');
        if continuation {
            let Some((section, item)) = last else {
                last_raw = trimmed.to_owned();
                continue;
            };
            let text = clean_text(trimmed);
            if contains_injection(&text) {
                injection += 1;
                sections[section].items.remove(item);
                last = None;
                last_raw = trimmed.to_owned();
                continue;
            }
            let is_task = sections[section].role == SectionRole::Task;
            let target = &mut sections[section].items[item];
            if target.text.is_empty() {
                target.text = text;
            } else {
                target.text.push(' ');
                target.text.push_str(&text);
            }
            if is_task {
                let (clean, task) = parse_task(&target.text);
                target.text = clean;
                target.task = task;
            }
        } else {
            if contains_injection(trimmed) {
                injection += 1;
            } else {
                let (text, quote) = split_quote(&clean_text(trimmed));
                let (text, task) = if role == SectionRole::Task {
                    parse_task(&text)
                } else {
                    (text, None)
                };
                push_item(&mut sections[current], text, role, task, quote);
                last = Some((current, sections[current].items.len() - 1));
                recovered += 1;
            }
        }
        last_raw = trimmed.to_owned();
    }

    let items: usize = sections.iter().map(|s| s.items.len()).sum();
    if items == 0 {
        return Err(SummaryError::InvalidOutput("no sections or items".into()));
    }
    let mut warnings = Vec::new();
    if recovered > 0 {
        warnings.push(Warning::ParserRecovered);
    }
    if injection > 0 {
        warnings.push(Warning::InjectionSuspected);
    }
    Ok(ParsedSummary {
        sections,
        warnings,
        parser_recovered: recovered,
        injection_flagged: injection,
    })
}

pub fn has_recognized_headings(output: &str, preset: &Preset) -> bool {
    output.lines().any(|line| {
        let candidate = heading(line.trim()).or_else(|| bare_heading(line.trim()));
        candidate
            .and_then(|name| resolve_heading(preset, &name))
            .is_some()
    })
}

fn parse_flat(output: &str, preset: &Preset) -> Result<ParsedSummary, SummaryError> {
    let mut doc = empty(preset);
    let default_index = doc
        .sections
        .iter()
        .position(|s| s.id == "brief")
        .unwrap_or(0);
    let decisions = section_index(preset, &["decisions"]);
    let task = section_index(preset, &["tasks", "next_steps", "todo"]);
    let questions = section_index(preset, &["questions"]);
    let mut recovered = 0usize;
    let mut injection = 0usize;

    for raw in output.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (label, text) = match label_line(trimmed) {
            Some((label, value)) => (Some(label), clean_text(value)),
            None => (None, clean_text(item_line(trimmed).unwrap_or(trimmed))),
        };
        if text.is_empty() {
            continue;
        }
        if contains_injection(&text) {
            injection += 1;
            continue;
        }
        let (text, quote) = split_quote(&text);
        let normalized = crate::text::normalize(&text);
        let decision_label = matches!(label.as_deref(), Some("решение") | Some("decision"));
        let task_label = matches!(
            label.as_deref(),
            Some("задача") | Some("что") | Some("task") | Some("what")
        );
        let target = if decision_label {
            decisions.unwrap_or(default_index)
        } else if task_label {
            task.unwrap_or(default_index)
        } else if cues::decision_cue(&normalized) {
            decisions.unwrap_or(default_index)
        } else if cues::question_line(&normalized) {
            questions.unwrap_or(default_index)
        } else if !cues::task_rejected(&normalized)
            && (cues::task_cue(&normalized) || looks_like_task(&normalized))
        {
            task.unwrap_or(default_index)
        } else {
            default_index
        };
        let role = doc.sections[target].role;
        let (text, task_meta) = if role == SectionRole::Task {
            let (text, task_meta) = parse_task(&text);
            (text, Some(task_meta.unwrap_or_else(empty_task)))
        } else {
            (text, None)
        };
        push_item(&mut doc.sections[target], text, role, task_meta, quote);
        recovered += 1;
    }
    let items: usize = doc.sections.iter().map(|s| s.items.len()).sum();
    if items == 0 {
        return Err(SummaryError::InvalidOutput("no sections or items".into()));
    }
    doc.parser_recovered = recovered;
    doc.warnings.push(Warning::ParserRecovered);
    if injection > 0 {
        doc.injection_flagged = injection;
        doc.warnings.push(Warning::InjectionSuspected);
    }
    Ok(doc)
}

fn section_index(preset: &Preset, ids: &[&str]) -> Option<usize> {
    ids.iter()
        .find_map(|id| preset.sections.iter().position(|s| s.id == *id))
}

fn looks_like_task(normalized: &str) -> bool {
    (normalized.contains("кто ") || normalized.contains("срок ")) && cues::action_verb(normalized)
}

fn empty_task() -> TaskMeta {
    TaskMeta {
        assignee: None,
        deadline: None,
        deadline_ref: None,
        deadline_source: DeadlineSource::Model,
    }
}

fn push_item(
    section: &mut ParsedSection,
    text: String,
    role: SectionRole,
    task: Option<TaskMeta>,
    quote: Option<String>,
) {
    if text.is_empty() {
        return;
    }
    section.items.push(ParsedItem {
        text,
        role,
        task,
        quote,
    });
}

fn apply_task_field(item: &mut ParsedItem, label: &str, value: &str) {
    let Some(task) = item.task.as_mut() else {
        return;
    };
    match label {
        "срок" | "deadline" | "due" => {
            if !is_no_deadline(value) {
                task.deadline = Some(value.to_owned());
                task.deadline_source = DeadlineSource::Model;
            }
        }
        "кто" | "ответственный" | "who" | "assignee" | "owner" if !is_no_assignee(value) =>
        {
            task.assignee = Some(value.to_owned());
        }
        _ => {}
    }
}

fn parse_task(text: &str) -> (String, Option<TaskMeta>) {
    let mut rest = Vec::new();
    let mut assignee = None;
    let mut deadline = None;
    let mut source = DeadlineSource::Model;
    for part in text.split(';') {
        let part = part.trim();
        if let Some(value) =
            strip_label(part, &["кто", "ответственный", "who", "assignee", "owner"])
        {
            if !is_no_assignee(value) {
                assignee = Some(value.to_owned());
            }
        } else if let Some(value) = strip_label(part, &["срок", "deadline", "due"]) {
            if !is_no_deadline(value) {
                deadline = Some(value.to_owned());
                source = DeadlineSource::Model;
            }
        } else if let Some(value) = strip_label(part, &["задача", "что", "task", "what"]) {
            rest.push(value.to_owned());
        } else if !part.is_empty() {
            rest.push(part.to_owned());
        }
    }
    let text = rest.join("; ").trim().to_owned();
    let task = if assignee.is_some() || deadline.is_some() {
        Some(TaskMeta {
            assignee,
            deadline,
            deadline_ref: None,
            deadline_source: source,
        })
    } else {
        Some(TaskMeta {
            assignee: None,
            deadline: None,
            deadline_ref: None,
            deadline_source: source,
        })
    };
    (text, task)
}

fn strip_label<'a>(part: &'a str, labels: &[&str]) -> Option<&'a str> {
    let (head, value) = part.split_once(':')?;
    if labels.contains(&head.trim().to_lowercase().as_str()) {
        Some(value.trim())
    } else {
        None
    }
}

fn is_no_deadline(value: &str) -> bool {
    matches!(
        normalize_placeholder(value).as_str(),
        "срок не назван" | "не назван" | "нет" | "no deadline" | "not stated" | "none"
    )
}

fn is_no_assignee(value: &str) -> bool {
    matches!(
        normalize_placeholder(value).as_str(),
        "не назначен"
            | "не назван"
            | "нет"
            | "not assigned"
            | "unassigned"
            | "no assignee"
            | "none"
    )
}

fn normalize_placeholder(value: &str) -> String {
    value
        .trim()
        .trim_matches('*')
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_quote(text: &str) -> (String, Option<String>) {
    if text.starts_with('«') && text.ends_with('»') && text.chars().count() >= 3 {
        let inner: String = text
            .chars()
            .skip(1)
            .take(text.chars().count() - 2)
            .collect();
        return (inner.clone(), Some(inner));
    }
    (text.to_owned(), None)
}

pub(crate) fn contains_injection(text: &str) -> bool {
    let low = text.to_lowercase();
    const MARKERS: &[&str] = &[
        "<|im_start|>",
        "<|im_end|>",
        "</think>",
        "### instruction",
        "ignore previous",
        "instructions",
        "system:",
        "assistant:",
        "<script",
        "<iframe",
        "<img",
        "javascript:",
        "href=",
        "http://",
        "https://",
        "<<<data_",
        "_begin>>>",
        "_end>>>",
    ];
    MARKERS.iter().any(|m| low.contains(m))
}

fn heading(line: &str) -> Option<String> {
    static H: OnceLock<Regex> = OnceLock::new();
    static B: OnceLock<Regex> = OnceLock::new();
    let h = H.get_or_init(|| Regex::new(r"^\s{0,3}#{2,4}\s*(.+?)\s*$").unwrap());
    let b = B.get_or_init(|| Regex::new(r"^\s*\*\*(.+?)\*\*\s*:?\s*$").unwrap());
    h.captures(line)
        .or_else(|| b.captures(line))
        .map(|c| c[1].trim().to_owned())
}

fn bare_heading(line: &str) -> Option<String> {
    static BARE: OnceLock<Regex> = OnceLock::new();
    let re = BARE.get_or_init(|| {
        Regex::new(r"^\s*\*{0,2}([A-Za-zА-Яа-яЁё][A-Za-zА-Яа-яЁё \-]{1,40})\*{0,2}\s*:\s*$")
            .unwrap()
    });
    re.captures(line).map(|c| c[1].trim().to_owned())
}

fn item_line(line: &str) -> Option<&str> {
    static I: OnceLock<Regex> = OnceLock::new();
    let re = I.get_or_init(|| Regex::new(r"^\s{0,3}(?:[-*+•]|\d{1,2}[.)])\s+(.+)$").unwrap());
    re.captures(line).map(|c| &line[c.get(1).unwrap().range()])
}

fn label_line(line: &str) -> Option<(String, &str)> {
    static L: OnceLock<Regex> = OnceLock::new();
    let re = L.get_or_init(|| {
        Regex::new(
            r"(?i)^\*{0,2}(Задача|Срок|Кто|Что|Решение|Ответственный|Task|Deadline|Due|Who|What|Decision|Assignee|Owner)\*{0,2}\s*:\s*\*{0,2}\s*(.+?)\s*\*{0,2}$",
        )
        .unwrap()
    });
    re.captures(line)
        .map(|c| (c[1].to_lowercase(), &line[c.get(2).unwrap().range()]))
}

fn clean_text(text: &str) -> String {
    text.trim()
        .trim_matches('*')
        .trim_matches('`')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
