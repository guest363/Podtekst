use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use crate::manifest::Manifest;
use crate::metrics::{clip_metrics, summarize, words_of, ClipMetrics, ClipResult, Summary};

fn vocabulary_key(items: &[String]) -> BTreeSet<String> {
    items
        .iter()
        .map(|item| words_of(item).join(" "))
        .filter(|item| !item.is_empty())
        .collect()
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn csv_line(fields: &[String]) -> String {
    fields
        .iter()
        .map(|field| csv_field(field))
        .collect::<Vec<_>>()
        .join(",")
}

fn opt(value: Option<f64>) -> String {
    value.map_or_else(String::new, |value| format!("{value:.6}"))
}

fn metrics_line(row: &ClipMetrics) -> String {
    csv_line(&[
        row.variant.clone(),
        row.group.clone(),
        row.file_id.clone(),
        row.language.clone(),
        row.engine.clone(),
        row.duration
            .map_or_else(String::new, |value| format!("{value:.3}")),
        row.ref_words.to_string(),
        row.wer_errors.to_string(),
        opt(row.wer),
        row.hyp_words.to_string(),
        row.term_expected.to_string(),
        row.term_observed.to_string(),
        row.term_tp.to_string(),
        row.term_fp.to_string(),
        row.term_fn.to_string(),
        opt(row.term_recall),
        opt(row.term_precision),
        row.clip_hit.map_or_else(String::new, |hit| hit.to_string()),
        row.false_insertions.to_string(),
        opt(row.false_insertions_per_1k),
        opt(row.wer_termnorm),
        row.ref_words_termnorm.to_string(),
        row.wer_errors_termnorm.to_string(),
        opt(row.wer_outside),
        row.ref_words_outside.to_string(),
        row.wer_errors_outside.to_string(),
        row.ctc_used
            .map_or_else(String::new, |used| used.to_string()),
        row.exit_code.to_string(),
        row.error.clone(),
    ])
}

fn summary_line(row: &Summary) -> String {
    csv_line(&[
        row.variant.clone(),
        row.group.clone(),
        row.files.to_string(),
        row.term_expected.to_string(),
        row.term_observed.to_string(),
        row.term_tp.to_string(),
        row.term_fp.to_string(),
        row.term_fn.to_string(),
        opt(row.term_recall),
        opt(row.term_precision),
        opt(row.clip_hit_rate),
        row.false_insertions.to_string(),
        opt(row.false_insertions_per_1k),
        row.hyp_words.to_string(),
        row.ref_words.to_string(),
        row.wer_errors.to_string(),
        opt(row.wer),
        row.ref_words_termnorm.to_string(),
        row.wer_errors_termnorm.to_string(),
        opt(row.wer_termnorm),
        row.ref_words_outside.to_string(),
        row.wer_errors_outside.to_string(),
        opt(row.wer_outside),
        opt(row.ctc_used_rate),
    ])
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(
        || "n/a".to_string(),
        |value| format!("{:.2}%", value * 100.0),
    )
}

fn number(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |value| format!("{value:.3}"))
}

fn render_report(summary: &[Summary], failures: usize, records: usize) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "\u{23} Словарь и hotwords");
    let _ = writeln!(output);
    let _ = writeln!(
        output,
        "Клипов: {records}. Провалов: {failures}. Метрики синтетические/локальные; абсолютные значения WER несравнимы с публичными наборами."
    );
    let _ = writeln!(output);
    let _ = writeln!(
        output,
        "| Вариант | Группа | Файлов | Recall | Precision | FI | FI/1k | WER | WER term-norm | WER вне терминов | CTC |"
    );
    let _ = writeln!(
        output,
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    );
    for row in summary {
        let _ = writeln!(
            output,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            row.variant,
            row.group,
            row.files,
            percent(row.term_recall),
            percent(row.term_precision),
            row.false_insertions,
            number(row.false_insertions_per_1k),
            percent(row.wer),
            percent(row.wer_termnorm),
            percent(row.wer_outside),
            percent(row.ctc_used_rate),
        );
    }
    let baseline = summary
        .iter()
        .find(|row| row.variant == "off" && row.group == "all");
    if let Some(baseline) = baseline {
        let deltas: Vec<&Summary> = summary
            .iter()
            .filter(|row| row.variant != "off" && row.group == "all")
            .collect();
        if !deltas.is_empty() {
            let _ = writeln!(output);
            let _ = writeln!(output, "\u{23}\u{23} Прирост к off");
            let _ = writeln!(output);
            let _ = writeln!(
                output,
                "| Вариант | Recall, п.п. | FI | WER, п.п. | WER вне терминов, п.п. |"
            );
            let _ = writeln!(output, "|---|---:|---:|---:|---:|");
            for row in deltas {
                let recall = match (row.term_recall, baseline.term_recall) {
                    (Some(left), Some(right)) => format!("{:+.2}", (left - right) * 100.0),
                    _ => "n/a".to_string(),
                };
                let wer = match (row.wer, baseline.wer) {
                    (Some(left), Some(right)) => format!("{:+.2}", (left - right) * 100.0),
                    _ => "n/a".to_string(),
                };
                let outside = match (row.wer_outside, baseline.wer_outside) {
                    (Some(left), Some(right)) => format!("{:+.2}", (left - right) * 100.0),
                    _ => "n/a".to_string(),
                };
                let _ = writeln!(
                    output,
                    "| {} | {} | {:+} | {} | {} |",
                    row.variant,
                    recall,
                    row.false_insertions as i64 - baseline.false_insertions as i64,
                    wer,
                    outside
                );
            }
        }
    }
    output
}

pub fn score(run: &Path, manifest: &Manifest, report: Option<&Path>) -> Result<(), String> {
    let mut metrics = Vec::new();
    let mut failures = Vec::new();
    let mut records = 0usize;
    for entry in std::fs::read_dir(run).map_err(|error| format!("{}: {error}", run.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let variant_dir = entry.path();
        if !variant_dir.is_dir() {
            continue;
        }
        let raw_path = variant_dir.join("raw.json");
        if !raw_path.is_file() {
            continue;
        }
        let bytes =
            std::fs::read(&raw_path).map_err(|error| format!("{}: {error}", raw_path.display()))?;
        let results: Vec<ClipResult> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", raw_path.display()))?;
        records += results.len();
        for result in results {
            if result.exit_code != 0 || !result.error.is_empty() {
                failures.push(result);
                continue;
            }
            if result.engine == "gigaam" {
                let ctc_expected = result.variant != "ctc-off";
                if ctc_expected && result.ctc_used != Some(true) {
                    failures.push(ClipResult {
                        error: "CTC did not run".to_string(),
                        ..result
                    });
                    continue;
                }
                if !ctc_expected && result.ctc_used == Some(true) {
                    failures.push(ClipResult {
                        error: "CTC ran in ctc-off".to_string(),
                        ..result
                    });
                    continue;
                }
            }
            if result.variant != "off"
                && vocabulary_key(&result.hotwords_applied) != vocabulary_key(&manifest.vocabulary)
            {
                failures.push(ClipResult {
                    error: "hotwords_applied mismatch".to_string(),
                    ..result
                });
                continue;
            }
            match clip_metrics(&result, &manifest.vocabulary) {
                Ok(row) => metrics.push(row),
                Err(error) => failures.push(ClipResult { error, ..result }),
            }
        }
    }
    if metrics.is_empty() {
        return Err("score: no successful clips".to_string());
    }
    let summary = summarize(&metrics);
    std::fs::write(
        run.join("metrics.csv"),
        metrics_header()
            + &metrics
                .iter()
                .map(metrics_line)
                .collect::<Vec<_>>()
                .join("\n")
            + "\n",
    )
    .map_err(|error| error.to_string())?;
    std::fs::write(
        run.join("summary.csv"),
        summary_header()
            + &summary
                .iter()
                .map(summary_line)
                .collect::<Vec<_>>()
                .join("\n")
            + "\n",
    )
    .map_err(|error| error.to_string())?;
    let failed = failures
        .iter()
        .map(|result| {
            csv_line(&[
                result.variant.clone(),
                result.file_id.clone(),
                result.exit_code.to_string(),
                result.error.clone(),
            ])
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        run.join("failures.csv"),
        "variant,file_id,exit_code,error\n".to_string() + &failed + "\n",
    )
    .map_err(|error| error.to_string())?;
    let report_path = report.map_or_else(|| run.join("report.md"), |path| path.to_path_buf());
    std::fs::write(
        &report_path,
        render_report(&summary, failures.len(), records),
    )
    .map_err(|error| format!("{}: {error}", report_path.display()))?;
    println!("score: {}", report_path.display());
    Ok(())
}

fn metrics_header() -> String {
    "variant,group,file_id,language,engine,duration,ref_words,wer_errors,wer,hyp_words,term_expected,term_observed,term_tp,term_fp,term_fn,term_recall,term_precision,clip_hit,false_insertions,false_insertions_per_1k,wer_termnorm,ref_words_termnorm,wer_errors_termnorm,wer_outside,ref_words_outside,wer_errors_outside,ctc_used,exit_code,error\n".to_string()
}

fn summary_header() -> String {
    "variant,group,files,term_expected,term_observed,term_tp,term_fp,term_fn,term_recall,term_precision,clip_hit_rate,false_insertions,false_insertions_per_1k,hyp_words,ref_words,wer_errors,wer,ref_words_termnorm,wer_errors_termnorm,wer_termnorm,ref_words_outside,wer_errors_outside,wer_outside,ctc_used_rate\n".to_string()
}
