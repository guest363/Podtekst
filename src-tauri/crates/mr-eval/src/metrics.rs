use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

pub const TERM_TOKEN: &str = "zzztermzzz";

pub fn normalize_word(value: &str) -> String {
    let normalized: String = value.nfkc().collect();
    normalized
        .to_lowercase()
        .replace('ё', "е")
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_string()
}

pub fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|raw| {
            let word = normalize_word(raw);
            (!word.is_empty()).then_some(word)
        })
        .collect()
}

pub fn words_of(value: &str) -> Vec<String> {
    tokens(value)
}

pub fn canonical_terms(vocabulary: &[String]) -> Vec<String> {
    let mut terms = Vec::new();
    for raw in vocabulary {
        let term = raw
            .split_once("=>")
            .map_or(raw.as_str(), |(_, right)| right);
        let normalized = words_of(term).join(" ");
        if !normalized.is_empty() && !terms.contains(&normalized) {
            terms.push(normalized);
        }
    }
    terms
}

pub fn alias_map(vocabulary: &[String]) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    for raw in vocabulary {
        let Some((left, right)) = raw.split_once("=>") else {
            continue;
        };
        let alias = words_of(left).join(" ");
        let term = words_of(right).join(" ");
        if !alias.is_empty() && !term.is_empty() && alias != term {
            aliases.insert(alias, term);
        }
    }
    aliases
}

fn phrases(terms: &[String]) -> Vec<Vec<String>> {
    let mut phrases: Vec<Vec<String>> = terms
        .iter()
        .map(|term| term.split_whitespace().map(str::to_string).collect())
        .filter(|phrase: &Vec<String>| !phrase.is_empty())
        .collect();
    phrases.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then(right.join(" ").cmp(&left.join(" ")))
    });
    phrases.dedup();
    phrases
}

fn matches(tokens: &[String], terms: &[Vec<String>]) -> Vec<(usize, usize, Vec<String>)> {
    let ordered = phrases(
        &terms
            .iter()
            .map(|phrase| phrase.join(" "))
            .collect::<Vec<_>>(),
    );
    let mut result = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let mut hit = None;
        for phrase in &ordered {
            let end = index + phrase.len();
            if end <= tokens.len() && tokens[index..end] == phrase[..] {
                hit = Some((index, end, phrase.clone()));
                break;
            }
        }
        if let Some((start, end, phrase)) = hit {
            result.push((start, end, phrase));
            index = end;
        } else {
            index += 1;
        }
    }
    result
}

pub fn count_terms(tokens: &[String], terms: &[String]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = terms.iter().map(|term| (term.clone(), 0)).collect();
    for (_, _, phrase) in matches(tokens, &phrases(terms)) {
        *counts.entry(phrase.join(" ")).or_default() += 1;
    }
    counts
}

fn replace_phrases(tokens: &[String], terms: &[String], replacement: &str) -> Vec<String> {
    let found = matches(tokens, &phrases(terms));
    if found.is_empty() {
        return tokens.to_vec();
    }
    let mut result = Vec::new();
    let mut copied = 0;
    for (start, end, _) in found {
        result.extend(tokens[copied..start].iter().cloned());
        result.push(replacement.to_string());
        copied = end;
    }
    result.extend(tokens[copied..].iter().cloned());
    result
}

fn strip_phrases(tokens: &[String], terms: &[String]) -> Vec<String> {
    let found = matches(tokens, &phrases(terms));
    if found.is_empty() {
        return tokens.to_vec();
    }
    let mut result = Vec::new();
    let mut copied = 0;
    for (start, end, _) in found {
        result.extend(tokens[copied..start].iter().cloned());
        copied = end;
    }
    result.extend(tokens[copied..].iter().cloned());
    result
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Wer {
    pub errors: usize,
    pub ref_words: usize,
}

impl Wer {
    pub fn rate(self) -> Option<f64> {
        (self.ref_words > 0).then(|| self.errors as f64 / self.ref_words as f64)
    }
}

pub fn wer(reference: &[String], hypothesis: &[String]) -> Wer {
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut current = vec![0usize; hypothesis.len() + 1];
    for (row, reference_word) in reference.iter().enumerate() {
        current[0] = row + 1;
        for (column, hypothesis_word) in hypothesis.iter().enumerate() {
            let substitution = previous[column] + usize::from(reference_word != hypothesis_word);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    Wer {
        errors: previous[hypothesis.len()],
        ref_words: reference.len(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipResult {
    pub variant: String,
    pub group: String,
    pub file_id: String,
    pub language: String,
    pub engine: String,
    pub duration: Option<f64>,
    pub reference: String,
    pub hypothesis: String,
    pub expected_terms: Vec<String>,
    pub ctc_used: Option<bool>,
    #[serde(default)]
    pub hotwords_applied: Vec<String>,
    pub exit_code: i32,
    pub error: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClipMetrics {
    pub variant: String,
    pub group: String,
    pub file_id: String,
    pub language: String,
    pub engine: String,
    pub duration: Option<f64>,
    pub ref_words: usize,
    pub wer_errors: usize,
    pub wer: Option<f64>,
    pub hyp_words: usize,
    pub term_expected: usize,
    pub term_observed: usize,
    pub term_tp: usize,
    pub term_fp: usize,
    pub term_fn: usize,
    pub term_recall: Option<f64>,
    pub term_precision: Option<f64>,
    pub clip_hit: Option<bool>,
    pub false_insertions: usize,
    pub false_insertions_per_1k: Option<f64>,
    pub wer_termnorm: Option<f64>,
    pub ref_words_termnorm: usize,
    pub wer_errors_termnorm: usize,
    pub wer_outside: Option<f64>,
    pub ref_words_outside: usize,
    pub wer_errors_outside: usize,
    pub ctc_used: Option<bool>,
    pub exit_code: i32,
    pub error: String,
}

fn rate(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

pub fn clip_metrics(result: &ClipResult, vocabulary: &[String]) -> Result<ClipMetrics, String> {
    let canonical = canonical_terms(vocabulary);
    let aliases = alias_map(vocabulary);
    let reference = tokens(&result.reference);
    let hypothesis = tokens(&result.hypothesis);
    let mut expected: BTreeMap<String, usize> = BTreeMap::new();
    for term in &result.expected_terms {
        let key = words_of(term).join(" ");
        if key.is_empty() {
            return Err(format!("{}: empty expected term", result.file_id));
        }
        if !canonical.contains(&key) {
            return Err(format!(
                "{}: expected term absent from vocabulary",
                result.file_id
            ));
        }
        *expected.entry(key).or_default() += 1;
    }
    let observed = count_terms(&hypothesis, &canonical);
    let terms: Vec<String> = expected
        .keys()
        .chain(observed.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let tp: usize = terms
        .iter()
        .map(|term| {
            expected
                .get(term)
                .copied()
                .unwrap_or(0)
                .min(observed.get(term).copied().unwrap_or(0))
        })
        .sum();
    let fp: usize = terms
        .iter()
        .map(|term| {
            observed
                .get(term)
                .copied()
                .unwrap_or(0)
                .saturating_sub(expected.get(term).copied().unwrap_or(0))
        })
        .sum();
    let fn_count: usize = terms
        .iter()
        .map(|term| {
            expected
                .get(term)
                .copied()
                .unwrap_or(0)
                .saturating_sub(observed.get(term).copied().unwrap_or(0))
        })
        .sum();
    let expected_total: usize = expected.values().sum();
    let observed_total: usize = observed.values().sum();
    let raw = wer(&reference, &hypothesis);
    let term_phrases: Vec<String> = canonical.iter().chain(aliases.keys()).cloned().collect();
    let termnorm = wer(
        &replace_phrases(&reference, &term_phrases, TERM_TOKEN),
        &replace_phrases(&hypothesis, &term_phrases, TERM_TOKEN),
    );
    let outside = wer(
        &strip_phrases(&reference, &term_phrases),
        &strip_phrases(&hypothesis, &term_phrases),
    );
    Ok(ClipMetrics {
        variant: result.variant.clone(),
        group: result.group.clone(),
        file_id: result.file_id.clone(),
        language: result.language.clone(),
        engine: result.engine.clone(),
        duration: result.duration,
        ref_words: raw.ref_words,
        wer_errors: raw.errors,
        wer: raw.rate(),
        hyp_words: hypothesis.len(),
        term_expected: expected_total,
        term_observed: observed_total,
        term_tp: tp,
        term_fp: fp,
        term_fn: fn_count,
        term_recall: rate(tp, expected_total),
        term_precision: rate(tp, tp + fp),
        clip_hit: (expected_total > 0).then_some(fn_count == 0),
        false_insertions: fp,
        false_insertions_per_1k: rate(1000 * fp, hypothesis.len()),
        wer_termnorm: termnorm.rate(),
        ref_words_termnorm: termnorm.ref_words,
        wer_errors_termnorm: termnorm.errors,
        wer_outside: outside.rate(),
        ref_words_outside: outside.ref_words,
        wer_errors_outside: outside.errors,
        ctc_used: result.ctc_used,
        exit_code: result.exit_code,
        error: result.error.clone(),
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub variant: String,
    pub group: String,
    pub files: usize,
    pub term_expected: usize,
    pub term_observed: usize,
    pub term_tp: usize,
    pub term_fp: usize,
    pub term_fn: usize,
    pub term_recall: Option<f64>,
    pub term_precision: Option<f64>,
    pub clip_hit_rate: Option<f64>,
    pub false_insertions: usize,
    pub false_insertions_per_1k: Option<f64>,
    pub hyp_words: usize,
    pub ref_words: usize,
    pub wer_errors: usize,
    pub wer: Option<f64>,
    pub ref_words_termnorm: usize,
    pub wer_errors_termnorm: usize,
    pub wer_termnorm: Option<f64>,
    pub ref_words_outside: usize,
    pub wer_errors_outside: usize,
    pub wer_outside: Option<f64>,
    pub ctc_used_rate: Option<f64>,
}

fn aggregate(rows: &[&ClipMetrics], variant: &str, group: &str) -> Summary {
    let mut summary = Summary {
        variant: variant.to_string(),
        group: group.to_string(),
        files: rows.len(),
        ..Summary::default()
    };
    for row in rows {
        summary.term_expected += row.term_expected;
        summary.term_observed += row.term_observed;
        summary.term_tp += row.term_tp;
        summary.term_fp += row.term_fp;
        summary.term_fn += row.term_fn;
        summary.false_insertions += row.false_insertions;
        summary.hyp_words += row.hyp_words;
        summary.ref_words += row.ref_words;
        summary.wer_errors += row.wer_errors;
        summary.ref_words_termnorm += row.ref_words_termnorm;
        summary.wer_errors_termnorm += row.wer_errors_termnorm;
        summary.ref_words_outside += row.ref_words_outside;
        summary.wer_errors_outside += row.wer_errors_outside;
    }
    summary.term_recall = rate(summary.term_tp, summary.term_expected);
    summary.term_precision = rate(summary.term_tp, summary.term_tp + summary.term_fp);
    let hits: Vec<bool> = rows.iter().filter_map(|row| row.clip_hit).collect();
    summary.clip_hit_rate = rate(hits.iter().filter(|hit| **hit).count(), hits.len());
    summary.false_insertions_per_1k = rate(1000 * summary.false_insertions, summary.hyp_words);
    summary.wer = rate(summary.wer_errors, summary.ref_words);
    summary.wer_termnorm = rate(summary.wer_errors_termnorm, summary.ref_words_termnorm);
    summary.wer_outside = rate(summary.wer_errors_outside, summary.ref_words_outside);
    let ctc: Vec<bool> = rows.iter().filter_map(|row| row.ctc_used).collect();
    summary.ctc_used_rate = rate(ctc.iter().filter(|used| **used).count(), ctc.len());
    summary
}

pub fn summarize(rows: &[ClipMetrics]) -> Vec<Summary> {
    let mut variants: BTreeMap<String, Vec<&ClipMetrics>> = BTreeMap::new();
    for row in rows {
        variants.entry(row.variant.clone()).or_default().push(row);
    }
    let mut result = Vec::new();
    for (variant, rows) in variants {
        result.push(aggregate(&rows, &variant, "all"));
        for group in ["term", "lookalike", "ordinary"] {
            let selected: Vec<&ClipMetrics> = rows
                .iter()
                .copied()
                .filter(|row| row.group == group)
                .collect();
            if !selected.is_empty() {
                result.push(aggregate(&selected, &variant, group));
            }
        }
        let no_term: Vec<&ClipMetrics> = rows
            .iter()
            .copied()
            .filter(|row| row.group != "term")
            .collect();
        if !no_term.is_empty() {
            result.push(aggregate(&no_term, &variant, "no-term"));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(group: &str, reference: &str, hypothesis: &str, expected: &[&str]) -> ClipResult {
        ClipResult {
            variant: "on".to_string(),
            group: group.to_string(),
            file_id: "clip".to_string(),
            language: "ru".to_string(),
            engine: "gigaam".to_string(),
            duration: Some(1.0),
            reference: reference.to_string(),
            hypothesis: hypothesis.to_string(),
            expected_terms: expected.iter().map(|term| term.to_string()).collect(),
            ctc_used: Some(true),
            hotwords_applied: Vec::new(),
            exit_code: 0,
            error: String::new(),
        }
    }

    #[test]
    fn normalizes_case_punctuation_and_yo() {
        assert_eq!(tokens("Ёлка, МЕРШИ!"), ["елка", "мерши"]);
        assert_eq!(tokens("секс-баг"), ["секс-баг"]);
        assert_eq!(tokens("Next.js"), ["next.js"]);
    }

    #[test]
    fn counts_terms_longest_first() {
        let words = tokens("альфа бета альфа");
        assert_eq!(
            count_terms(&words, &["альфа".into(), "бета".into()]),
            BTreeMap::from([("альфа".to_string(), 2), ("бета".to_string(), 1),])
        );
        assert_eq!(
            count_terms(&tokens("альфат"), &["альфа".into()]),
            BTreeMap::from([("альфа".to_string(), 0)])
        );
    }

    #[test]
    fn recall_and_false_insertions() {
        let metrics = clip_metrics(
            &result("term", "альфа и бета", "альфа альфа", &["альфа", "бета"]),
            &["альфа".into(), "бета".into()],
        )
        .unwrap();
        assert_eq!(
            (metrics.term_tp, metrics.term_fn, metrics.term_fp),
            (1, 1, 1)
        );
        assert_eq!(metrics.term_recall, Some(0.5));
        assert_eq!(metrics.term_precision, Some(0.5));
    }

    #[test]
    fn term_normalized_wer_handles_multiword_and_alias() {
        let metrics = clip_metrics(
            &result("term", "юзер флоу готов", "User Flow готов", &["User Flow"]),
            &["юзер флоу => User Flow".into()],
        )
        .unwrap();
        assert_eq!(metrics.term_tp, 1);
        assert_eq!(metrics.wer_termnorm, Some(0.0));
    }

    #[test]
    fn no_term_wer_is_aggregated_separately() {
        let rows = vec![
            clip_metrics(&result("ordinary", "план", "альфа", &[]), &["альфа".into()]).unwrap(),
            clip_metrics(
                &result("term", "альфа", "альфа", &["альфа"]),
                &["альфа".into()],
            )
            .unwrap(),
        ];
        let summary = summarize(&rows);
        let no_term = summary.iter().find(|row| row.group == "no-term").unwrap();
        let term = summary.iter().find(|row| row.group == "term").unwrap();
        assert_eq!(no_term.false_insertions, 1);
        assert_eq!(term.wer, Some(0.0));
        assert_eq!(no_term.wer, Some(1.0));
    }

    #[test]
    fn wer_counts_substitutions_insertions_and_deletions() {
        let reference = tokens("один два три");
        let hypothesis = tokens("один четыре");
        assert_eq!(
            wer(&reference, &hypothesis),
            Wer {
                errors: 2,
                ref_words: 3
            }
        );
        assert_eq!(wer(&reference, &reference).rate(), Some(0.0));
    }
}
