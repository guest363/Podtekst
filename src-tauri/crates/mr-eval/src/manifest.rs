use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::metrics::{canonical_terms, count_terms, tokens};

#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub vocabulary: Vec<String>,
    pub records: Vec<Record>,
}

#[derive(Debug, Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(default)]
    pub audio: String,
    #[serde(default)]
    pub ref_text: Option<String>,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub expected_terms: Vec<String>,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub duration: Option<f64>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn validate(&self, base: &Path) -> Result<(), String> {
        if self.records.is_empty() {
            return Err("manifest: records[] is empty".to_string());
        }
        let canonical = canonical_terms(&self.vocabulary);
        let allowed: BTreeSet<&str> = canonical.iter().map(String::as_str).collect();
        let mut ids = BTreeSet::new();
        for record in &self.records {
            if record.id.trim().is_empty() {
                return Err("manifest: empty record id".to_string());
            }
            if !ids.insert(record.id.as_str()) {
                return Err(format!("manifest: duplicate record id {}", record.id));
            }
            if !matches!(record.group.as_str(), "term" | "lookalike" | "ordinary") {
                return Err(format!(
                    "{}: group must be term, lookalike or ordinary",
                    record.id
                ));
            }
            let reference = record.reference_text(base)?;
            let mut expected = Vec::new();
            for term in &record.expected_terms {
                let key = crate::metrics::words_of(term).join(" ");
                if key.is_empty() {
                    return Err(format!("{}: empty expected term", record.id));
                }
                if !allowed.contains(key.as_str()) {
                    return Err(format!(
                        "{}: expected term {key} is absent from vocabulary",
                        record.id
                    ));
                }
                expected.push(key);
            }
            let expected_counts = expected.iter().fold(
                std::collections::BTreeMap::<String, usize>::new(),
                |mut counts, term| {
                    *counts.entry(term.clone()).or_default() += 1;
                    counts
                },
            );
            let actual = count_terms(&tokens(&reference), &canonical);
            let actual: std::collections::BTreeMap<String, usize> =
                actual.into_iter().filter(|(_, count)| *count > 0).collect();
            if expected_counts != actual {
                return Err(format!(
                    "{}: expected_terms do not match the reference text",
                    record.id
                ));
            }
            if record.group == "term" && expected_counts.is_empty() {
                return Err(format!("{}: term clip needs expected_terms", record.id));
            }
            if record.group != "term" && !expected_counts.is_empty() {
                return Err(format!(
                    "{}: non-term clip must not expect terms",
                    record.id
                ));
            }
        }
        Ok(())
    }
}

impl Record {
    pub fn reference_text(&self, base: &Path) -> Result<String, String> {
        if let Some(reference) = &self.reference {
            return Ok(reference.clone());
        }
        let Some(path) = &self.ref_text else {
            return Err(format!("{}: ref_text is required", self.id));
        };
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            base.join(path)
        };
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(records: Vec<Record>) -> Manifest {
        Manifest {
            name: "test".to_string(),
            language: "ru".to_string(),
            engine: "gigaam".to_string(),
            vocabulary: vec!["альфа".to_string(), "бета".to_string()],
            records,
        }
    }

    fn record(id: &str, group: &str, reference: &str, expected: &[&str]) -> Record {
        Record {
            id: id.to_string(),
            audio: String::new(),
            ref_text: None,
            reference: Some(reference.to_string()),
            group: group.to_string(),
            expected_terms: expected.iter().map(|term| term.to_string()).collect(),
            language: String::new(),
            engine: String::new(),
            duration: None,
        }
    }

    #[test]
    fn validates_expected_terms_against_reference() {
        let good = manifest(vec![record(
            "term",
            "term",
            "альфа и бета",
            &["альфа", "бета"],
        )]);
        assert!(good.validate(Path::new(".")).is_ok());
        let drift = manifest(vec![record("term", "term", "альфа", &["бета"])]);
        assert!(drift.validate(Path::new(".")).is_err());
        let ordinary = manifest(vec![record("ordinary", "ordinary", "альфа", &[])]);
        assert!(ordinary.validate(Path::new(".")).is_err());
    }

    #[test]
    fn rejects_empty_groups_and_duplicate_ids() {
        let bad_group = manifest(vec![record("x", "call", "план", &[])]);
        assert!(bad_group.validate(Path::new(".")).is_err());
        let duplicate = manifest(vec![
            record("x", "ordinary", "план", &[]),
            record("x", "ordinary", "план", &[]),
        ]);
        assert!(duplicate.validate(Path::new(".")).is_err());
    }
}
