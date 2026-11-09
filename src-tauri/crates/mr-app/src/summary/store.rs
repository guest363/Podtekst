use std::path::{Path, PathBuf};

use mr_summarize::artifact::{parse_artifact, SummaryArtifact};

use crate::config::save_dir;

#[derive(Debug, Clone)]
pub struct Paths {
    pub transcript: PathBuf,
    pub json: PathBuf,
    pub md: PathBuf,
    pub parent: PathBuf,
    pub relative: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Missing,
    Fresh,
    Stale,
    Unsupported,
    Corrupt,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unsupported => "unsupported",
            Self::Corrupt => "corrupt",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub state: State,
    pub artifact: Option<SummaryArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NoFile,
    Outside,
    NoTranscript,
    NoText,
    TooShort,
    OutputCapped,
    Io(String),
}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoFile => "no_file",
            Self::Outside => "outside_save_dir",
            Self::NoTranscript => "no_transcript",
            Self::NoText => "no_text",
            Self::TooShort => "too_short",
            Self::OutputCapped => "output_capped",
            Self::Io(_) => "internal",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::NoFile => 400,
            Self::Outside | Self::NoTranscript => 404,
            Self::NoText | Self::TooShort | Self::OutputCapped => 422,
            Self::Io(_) => 500,
        }
    }
}

pub fn summary_paths(parent: &Path, stem: &str, relative: String) -> Paths {
    Paths {
        transcript: parent.join(format!("{stem}.transcript.json")),
        json: parent.join(format!("{stem}.summary.json")),
        md: parent.join(format!("{stem}.summary.md")),
        parent: parent.to_path_buf(),
        relative,
    }
}

/// Тот же резолвер, что у `serve_transcript`: относительное имя — от папки
/// записей, абсолютное — только внутри неё; наружу симлинком — отказ.
pub fn resolve(file: &str) -> Result<Paths, StoreError> {
    let file = file.trim();
    if file.is_empty() {
        return Err(StoreError::NoFile);
    }
    let raw = PathBuf::from(file);
    let audio = if raw.is_absolute() {
        raw
    } else {
        save_dir().join(&raw)
    };
    if audio.file_name().is_none() || !crate::http::media::inside_save_dir(&audio) {
        return Err(StoreError::Outside);
    }
    let stem = audio
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if stem.is_empty() {
        return Err(StoreError::NoFile);
    }
    let parent = audio.parent().unwrap_or_else(|| Path::new("."));
    let base = save_dir();
    let relative = audio
        .strip_prefix(&base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| {
            audio
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });
    let paths = summary_paths(parent, &stem, relative);
    if !paths.transcript.is_file() {
        return Err(StoreError::NoTranscript);
    }
    Ok(paths)
}

/// Схема: чужая major (новее или неизвестная) — `unsupported`, битый
/// JSON/shape — `corrupt`. Ни то, ни другое не удаляется автоматически.
pub fn load(paths: &Paths, current_rev: &str) -> Loaded {
    let Ok(text) = std::fs::read_to_string(&paths.json) else {
        return Loaded {
            state: State::Missing,
            artifact: None,
        };
    };
    let value = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(&paths.json),
                error = error.to_string();
                "ev=json.load"
            );
            return Loaded {
                state: State::Corrupt,
                artifact: None,
            };
        }
    };
    let schema = value
        .get("schema")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let major = schema
        .strip_prefix("podtekst.summary/v")
        .and_then(|tail| tail.parse::<u32>().ok());
    if major != Some(mr_summarize::artifact::SCHEMA_MAJOR) {
        return Loaded {
            state: State::Unsupported,
            artifact: None,
        };
    }
    match parse_artifact(&text) {
        Ok(artifact) => {
            let state = if artifact.source.rev == current_rev {
                State::Fresh
            } else {
                State::Stale
            };
            Loaded {
                state,
                artifact: Some(artifact),
            }
        }
        Err(_) => Loaded {
            state: State::Corrupt,
            artifact: None,
        },
    }
}

/// Состояние файла без сравнения rev: нужен на commit point, чтобы бэкапить
/// corrupt/newer только когда новый артефакт действительно пишется.
pub fn inspect(paths: &Paths) -> State {
    let Ok(text) = std::fs::read_to_string(&paths.json) else {
        return State::Missing;
    };
    let value = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(&paths.json),
                error = error.to_string();
                "ev=json.load"
            );
            return State::Corrupt;
        }
    };
    let schema = value
        .get("schema")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if schema != mr_summarize::artifact::SCHEMA {
        return State::Unsupported;
    }
    if parse_artifact(&text).is_ok() {
        State::Fresh
    } else {
        State::Corrupt
    }
}

pub fn write(paths: &Paths, artifact: &SummaryArtifact) -> Result<bool, StoreError> {
    let json = artifact.to_json().map_err(|_| StoreError::OutputCapped)?;
    if json.len() > mr_summarize::artifact::MAX_JSON_BYTES {
        return Err(StoreError::OutputCapped);
    }
    crate::fsutil::write_atomic_private(&paths.json, json.as_bytes())
        .map_err(|e| StoreError::Io(e.to_string()))?;
    let md = artifact.to_markdown();
    if md.len() > mr_summarize::artifact::MAX_MD_BYTES {
        return Err(StoreError::OutputCapped);
    }
    Ok(crate::fsutil::write_atomic_private(&paths.md, md.as_bytes()).is_ok())
}

fn backups(prefix: &Path, kind: &str) -> Vec<PathBuf> {
    let parent = prefix.parent().unwrap_or_else(|| Path::new("."));
    let name = prefix.file_name().unwrap_or_default().to_string_lossy();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy();
                    n.starts_with(&format!("{name}.{kind}."))
                })
                .unwrap_or(false)
        })
        .collect();
    out.sort();
    out
}

fn backup(paths: &Paths, kind: &str) -> Result<(), StoreError> {
    if !paths.json.exists() {
        return Ok(());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut target = PathBuf::from(format!("{}.{kind}.{stamp}", paths.json.display()));
    let mut n = 1;
    while target.exists() {
        target = PathBuf::from(format!("{}.{kind}.{stamp}.{n}", paths.json.display()));
        n += 1;
    }
    std::fs::rename(&paths.json, &target).map_err(|e| StoreError::Io(e.to_string()))?;
    let old = backups(&paths.json, kind);
    for stale in old.iter().rev().skip(3) {
        let _ = std::fs::remove_file(stale);
    }
    Ok(())
}

pub fn backup_corrupt(paths: &Paths) -> Result<(), StoreError> {
    backup(paths, "corrupt")
}

pub fn backup_newer(paths: &Paths) -> Result<(), StoreError> {
    backup(paths, "newer")
}

pub fn has_summary(parent: &Path, stem: &str) -> bool {
    parent.join(format!("{stem}.summary.json")).is_file()
        && parent.join(format!("{stem}.summary.md")).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_summarize::artifact::SCHEMA;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mr-summary-store-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn artifact_text(rev: &str) -> String {
        format!(
            r#"{{"schema":"{SCHEMA}","schema_minor":0,"id":"id","generated_at":"t",
            "source":{{"schema":1,"rev":"{rev}","utterances":1,"chars":10,"language":"ru","duration_sec":1.0}},
            "classification":{{"preset_id":null,"confidence":null,"alternatives":[],"signals":[],
            "ambiguous":false,"style":"monologue","language":"ru","classified_at":"t",
            "heuristic_version":1,"spec_version":"classify-spec.v1","user_override":false}},
            "preset":{{"id":"meeting","version":1,"label":"Встреча","confirmed_sensitive":false}},
            "detail":"normal","output_language":"ru",
            "generator":{{"runtime":"llama.cpp/b12182","runtime_commit":"fd27b2034","backend":"sidecar",
            "model_id":"m","model_title":"M","model_sha256":"s","quant":"Q4_K_M","context":8192,
            "threads":2,"temperature":0.0,"top_p":0.9,"repeat_penalty":1.1,"max_tokens":768,
            "seed":42,"prompt_version":"meeting-v2","duration_ms":1,
            "timings_ms":{{"load":0,"prepare":0,"generate":1,"verify":0,"total":1}}}},
            "summary":{{"sections":[],"warnings":[],"truncated":false,"coverage":1.0,"items_total":0}},
            "provenance":{{"mode":"single_pass","chunks":1,"reduce_levels":0,"items_dropped":0,
            "soft_claims_dropped":0,"parser_retries":0,"parser_recovered":0,"retry_reason":null}}}}
        "#
        )
    }

    #[test]
    fn states_missing_fresh_stale_corrupt_unsupported() {
        let dir = temp_dir("states");
        let paths = summary_paths(&dir, "rec", "rec.wav".to_string());
        std::fs::write(&paths.json, artifact_text("rev1")).unwrap();
        assert_eq!(load(&paths, "rev1").state, State::Fresh);
        assert_eq!(load(&paths, "rev2").state, State::Stale);

        std::fs::write(&paths.json, "{broken").unwrap();
        assert_eq!(load(&paths, "rev1").state, State::Corrupt);

        let v2 = artifact_text("rev1").replace(SCHEMA, "podtekst.summary/v2");
        std::fs::write(&paths.json, v2).unwrap();
        assert_eq!(load(&paths, "rev1").state, State::Unsupported);
        assert!(paths.json.exists(), "unsupported не удаляется");

        std::fs::remove_file(&paths.json).unwrap();
        assert_eq!(load(&paths, "rev1").state, State::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_backup_keeps_at_most_three() {
        let dir = temp_dir("backup");
        let paths = summary_paths(&dir, "rec", "rec.wav".to_string());
        for _ in 0..5 {
            std::fs::write(&paths.json, "{broken").unwrap();
            backup_corrupt(&paths).unwrap();
        }
        let count = std::fs::read_dir(&dir).unwrap().flatten().count();
        assert!(count <= 3, "бэкапов больше трёх: {count}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn has_summary_requires_both_files() {
        let dir = temp_dir("has");
        let paths = summary_paths(&dir, "rec", "rec.wav".to_string());
        assert!(!has_summary(&dir, "rec"));
        std::fs::write(&paths.json, b"{}").unwrap();
        assert!(!has_summary(&dir, "rec"));
        std::fs::write(&paths.md, b"#").unwrap();
        assert!(has_summary(&dir, "rec"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
