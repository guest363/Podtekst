use std::sync::OnceLock;

use regex::Regex;

use crate::artifact::{DeadlineSource, SummaryRef, Warning};
use crate::text::{normalize, Utterance};

#[derive(Debug, Clone, PartialEq)]
pub struct DeadlineMatch {
    pub phrase: String,
    pub utterance_index: usize,
    pub start: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeadlineOutcome {
    pub deadline: Option<String>,
    pub reference: Option<SummaryRef>,
    pub source: DeadlineSource,
    pub warning: Option<Warning>,
}

pub fn find(utterances: &[Utterance], indices: &[usize]) -> Option<DeadlineMatch> {
    let mut best: Option<(usize, usize, usize, DeadlineMatch)> = None;
    for (rank, index) in indices.iter().enumerate() {
        let Some(utterance) = utterances.get(*index) else {
            continue;
        };
        for (start, len, end) in matches_in(&utterance.text) {
            let better = best
                .as_ref()
                .is_none_or(|(best_rank, best_start, best_len, _)| {
                    rank < *best_rank
                        || (rank == *best_rank
                            && (start < *best_start || (start == *best_start && len > *best_len)))
                });
            if better {
                best = Some((
                    rank,
                    start,
                    len,
                    DeadlineMatch {
                        phrase: utterance.text[start..end].to_owned(),
                        utterance_index: *index,
                        start: utterance.start,
                    },
                ));
            }
        }
    }
    best.map(|(_, _, _, m)| m)
}

pub fn find_in_text(text: &str, utterance_index: usize, start: f64) -> Option<DeadlineMatch> {
    matches_in(text)
        .into_iter()
        .next()
        .map(|(s, _, e)| DeadlineMatch {
            phrase: text[s..e].to_owned(),
            utterance_index,
            start,
        })
}

fn matches_in(text: &str) -> Vec<(usize, usize, usize)> {
    let mut found = Vec::new();
    for pattern in patterns() {
        for m in pattern.find_iter(text) {
            if m.start() == m.end() {
                continue;
            }
            found.push((m.start(), m.end() - m.start(), m.end()));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    found
}

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"\b(до|к|ко|в|во|на)\s+(понедельник[ау]?|вторник[ау]?|сред[ауы]|четверг[ау]?|пятниц[ауые]|суббот[ауы]|воскресень[еяю])\b",
            r"\b(сегодня|завтра|послезавтра|на этой неделе|на следующей неделе|к концу (недели|месяца|квартала|года)|в конце (недели|месяца)|через\s+\d+\s+(день|дня|дней|неделю|недели|недель|месяц|месяца|месяцев))\b",
            r"\b\d{1,2}[./-]\d{1,2}([./-]\d{2,4})?\b",
            r"\b\d{1,2}\s+(января|февраля|марта|апреля|мая|июня|июля|августа|сентября|октября|ноября|декабря)\b",
            r"\b(январь|февраль|март|апрель|май|июнь|июль|август|сентябрь|октябрь|ноябрь|декабрь)\s+\d{4}\b",
            r"\b(до|к)\s+\d{1,2}[:.]\d{2}\b",
            r"\b(до конца (квартала|года)|в (первом|втором|третьем|четвёртом) квартале|Q[1-4])\b",
        ]
        .iter()
        .map(|p| Regex::new(&format!("(?i){p}")).expect("deadline regex"))
        .collect()
    })
}

pub fn reconcile(model: Option<&str>, extracted: Option<&DeadlineMatch>) -> DeadlineOutcome {
    match (model, extracted) {
        (Some(model), Some(found)) => {
            if normalize(model) == normalize(&found.phrase) {
                DeadlineOutcome {
                    deadline: Some(model.to_owned()),
                    reference: Some(reference(found)),
                    source: DeadlineSource::Extracted,
                    warning: None,
                }
            } else {
                DeadlineOutcome {
                    deadline: Some(found.phrase.clone()),
                    reference: Some(reference(found)),
                    source: DeadlineSource::Extracted,
                    warning: Some(Warning::DeadlineRecovered),
                }
            }
        }
        (None, Some(found)) => DeadlineOutcome {
            deadline: Some(found.phrase.clone()),
            reference: Some(reference(found)),
            source: DeadlineSource::Extracted,
            warning: Some(Warning::DeadlineRecovered),
        },
        (Some(model), None) => {
            let placeholder = model.trim().is_empty()
                || normalize(model) == "срок не назван"
                || normalize(model) == "не назван";
            DeadlineOutcome {
                deadline: None,
                reference: None,
                source: DeadlineSource::Model,
                warning: (!placeholder).then_some(Warning::DeadlineNotStated),
            }
        }
        (None, None) => DeadlineOutcome {
            deadline: None,
            reference: None,
            source: DeadlineSource::Model,
            warning: None,
        },
    }
}

fn reference(found: &DeadlineMatch) -> SummaryRef {
    SummaryRef {
        utterance_index: found.utterance_index,
        start: found.start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(text: &str) -> Utterance {
        Utterance {
            start: 0.0,
            end: 1.0,
            speaker: "A".into(),
            text: text.into(),
        }
    }

    #[test]
    fn recall_three_relative_and_one_absolute() {
        let utterances = [
            u("Сделаю отчёт до среды."),
            u("Позвоню завтра."),
            u("Проверю к концу недели."),
            u("Отправлю 14.11.2026."),
        ];
        let phrases: Vec<_> = (0..4)
            .map(|i| find(&utterances, &[i]).unwrap().phrase)
            .collect();
        assert_eq!(
            phrases,
            ["до среды", "завтра", "к концу недели", "14.11.2026"]
        );
    }

    #[test]
    fn negative_task_stays_null() {
        let utterances = [u("Проверю журнал ошибок.")];
        assert!(find(&utterances, &[0]).is_none());
        let outcome = reconcile(None, None);
        assert!(outcome.deadline.is_none());
        assert!(outcome.warning.is_none());
    }

    #[test]
    fn model_invented_deadline_is_removed() {
        let outcome = reconcile(Some("до пятницы"), None);
        assert!(outcome.deadline.is_none());
        assert_eq!(outcome.warning, Some(Warning::DeadlineNotStated));
    }

    #[test]
    fn extracted_phrase_overrides_model() {
        let utterances = [u("Сделаю до среды.")];
        let found = find(&utterances, &[0]).unwrap();
        let outcome = reconcile(Some("до четверга"), Some(&found));
        assert_eq!(outcome.deadline.as_deref(), Some("до среды"));
        assert_eq!(outcome.warning, Some(Warning::DeadlineRecovered));
    }

    #[test]
    fn leftmost_longest_prefers_earlier_phrase() {
        let utterances = [u("Сделаю до среды или 14.11.2026.")];
        assert_eq!(find(&utterances, &[0]).unwrap().phrase, "до среды");
    }

    #[test]
    fn supplied_ref_order_wins_over_utterance_index() {
        let utterances = [u("Сделаю до среды."), u("Позвоню в пятницу.")];
        let found = find(&utterances, &[1, 0]).unwrap();
        assert_eq!(found.utterance_index, 1);
        assert_eq!(found.phrase, "в пятницу");
    }
}
