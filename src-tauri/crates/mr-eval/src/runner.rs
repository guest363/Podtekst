use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::manifest::{Manifest, Record};
use crate::metrics::ClipResult;

pub struct Variant {
    pub name: String,
    pub hotwords: Vec<String>,
    pub ctc_enabled: bool,
    pub ctc_mode: &'static str,
    pub env: Vec<(String, String)>,
}

impl Variant {
    pub fn parse(spec: &str, vocabulary: &[String]) -> Result<Self, String> {
        let (name, hotwords, ctc_enabled, ctc_mode, env) = match spec {
            "off" => ("off", Vec::new(), true, "auto", Vec::new()),
            "on" => ("on", vocabulary.to_vec(), true, "auto", Vec::new()),
            "ctc-off" => ("ctc-off", vocabulary.to_vec(), false, "off", Vec::new()),
            _ if spec.starts_with("on-b") => {
                let raw = &spec["on-b".len()..];
                let bonus: f32 = raw
                    .parse()
                    .ok()
                    .filter(|value: &f32| value.is_finite() && (0.0..=10.0).contains(value))
                    .ok_or_else(|| format!("variant {spec}: bonus must be 0..10"))?;
                (
                    spec,
                    vocabulary.to_vec(),
                    true,
                    "auto",
                    vec![
                        ("MR_HOTWORD_BENCH".to_string(), "1".to_string()),
                        ("MR_HOTWORD_BONUS".to_string(), format!("{bonus}")),
                    ],
                )
            }
            _ if spec.starts_with("on-c") => {
                let raw = &spec["on-c".len()..];
                let confidence: f64 = raw
                    .parse()
                    .ok()
                    .filter(|value: &f64| value.is_finite() && (0.0..=1.0).contains(value))
                    .ok_or_else(|| format!("variant {spec}: confidence must be 0..1"))?;
                (
                    spec,
                    vocabulary.to_vec(),
                    true,
                    "auto",
                    vec![
                        ("MR_HOTWORD_BENCH".to_string(), "1".to_string()),
                        ("MR_HOTWORD_CONFIDENCE".to_string(), format!("{confidence}")),
                    ],
                )
            }
            _ => return Err(format!("unknown variant: {spec}")),
        };
        Ok(Self {
            name: name.to_string(),
            hotwords,
            ctc_enabled,
            ctc_mode,
            env,
        })
    }
}

struct ConfigGuard {
    path: PathBuf,
    previous: Option<Vec<u8>>,
}

impl ConfigGuard {
    fn apply(data_dir: &Path, hotwords: &[String], ctc_enabled: bool) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
        let path = data_dir.join("config.json");
        let previous = std::fs::read(&path).ok();
        let mut config: Value = previous
            .as_deref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or_else(|| json!({}));
        if !config.is_object() {
            config = json!({});
        }
        config["hotwords"] = json!(hotwords);
        config["ctc_enabled"] = json!(ctc_enabled);
        write_atomic(
            &path,
            serde_json::to_vec_pretty(&config)
                .map_err(|error| error.to_string())?
                .as_slice(),
        )?;
        Ok(Self { path, previous })
    }
}

impl Drop for ConfigGuard {
    fn drop(&mut self) {
        let _ = match &self.previous {
            Some(bytes) => write_atomic(&self.path, bytes),
            None => std::fs::remove_file(&self.path).map_err(|error| error.to_string()),
        };
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

pub fn default_app() -> PathBuf {
    let name = if cfg!(windows) {
        "podtekst.exe"
    } else {
        "podtekst"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/release")
        .join(name)
}

pub struct RunOptions<'a> {
    pub manifest_path: &'a Path,
    pub data_dir: &'a Path,
    pub app: &'a Path,
    pub out: &'a Path,
    pub variants: &'a [String],
    pub engine: &'a str,
    pub language: &'a str,
    pub timeout: Duration,
}

struct RecordOptions<'a> {
    base: &'a Path,
    variant: &'a Variant,
    variant_dir: &'a Path,
    data_dir: &'a Path,
    app: &'a Path,
    engine: &'a str,
    language: &'a str,
    timeout: Duration,
}

pub fn run(options: RunOptions<'_>) -> Result<(), String> {
    let manifest = Manifest::load(options.manifest_path)?;
    let base = options
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    manifest.validate(base)?;
    let engine = if options.engine.is_empty() {
        if manifest.engine.is_empty() {
            "auto"
        } else {
            manifest.engine.as_str()
        }
    } else {
        options.engine
    };
    let language = if options.language.is_empty() {
        if manifest.language.is_empty() {
            "auto"
        } else {
            manifest.language.as_str()
        }
    } else {
        options.language
    };
    println!(
        "manifest: {} ({} records, {} terms)",
        manifest.name,
        manifest.records.len(),
        manifest.vocabulary.len()
    );
    std::fs::create_dir_all(options.out).map_err(|error| error.to_string())?;
    for spec in options.variants {
        let variant = Variant::parse(spec, &manifest.vocabulary)?;
        let _guard = ConfigGuard::apply(options.data_dir, &variant.hotwords, variant.ctc_enabled)?;
        let variant_dir = options.out.join(&variant.name);
        std::fs::create_dir_all(&variant_dir).map_err(|error| error.to_string())?;
        let record_options = RecordOptions {
            base,
            variant: &variant,
            variant_dir: &variant_dir,
            data_dir: options.data_dir,
            app: options.app,
            engine,
            language,
            timeout: options.timeout,
        };
        let mut results = Vec::new();
        for record in &manifest.records {
            results.push(run_record(record, &record_options));
        }
        let bytes = serde_json::to_vec_pretty(&results).map_err(|error| error.to_string())?;
        std::fs::write(variant_dir.join("raw.json"), bytes).map_err(|error| error.to_string())?;
    }
    crate::report::score(options.out, &manifest, None)
}

fn run_record(record: &Record, options: &RecordOptions<'_>) -> ClipResult {
    let mut result = ClipResult {
        variant: options.variant.name.clone(),
        group: record.group.clone(),
        file_id: record.id.clone(),
        language: if record.language.is_empty() {
            options.language.to_string()
        } else {
            record.language.clone()
        },
        engine: if record.engine.is_empty() {
            options.engine.to_string()
        } else {
            record.engine.clone()
        },
        duration: record.duration,
        reference: String::new(),
        hypothesis: String::new(),
        expected_terms: record.expected_terms.clone(),
        ctc_used: None,
        hotwords_applied: Vec::new(),
        exit_code: 1,
        error: String::new(),
    };
    if record.audio.trim().is_empty() {
        result.error = "audio is required".to_string();
        return result;
    }
    let source = options.base.join(&record.audio);
    let Some(extension) = source.extension() else {
        result.error = "audio has no extension".to_string();
        return result;
    };
    let audio_dir = options.variant_dir.join("audio");
    if let Err(error) = std::fs::create_dir_all(&audio_dir) {
        result.error = error.to_string();
        return result;
    }
    let audio = audio_dir.join(format!("{}.{}", record.id, extension.to_string_lossy()));
    if let Err(error) = std::fs::copy(&source, &audio) {
        result.error = format!("{}: {error}", source.display());
        return result;
    }
    let stem = audio.with_extension("");
    for suffix in ["transcript.json", "md", "txt", "perf.json"] {
        let _ = std::fs::remove_file(stem.with_extension(suffix));
    }
    result.reference = match record.reference_text(options.base) {
        Ok(reference) => reference,
        Err(error) => {
            result.error = error;
            return result;
        }
    };
    let mut command = Command::new(options.app);
    command
        .arg("--transcribe")
        .arg(&audio)
        .arg("--engine")
        .arg(&result.engine)
        .arg("--language")
        .arg(&result.language)
        .arg("--ctc")
        .arg(options.variant.ctc_mode)
        .env("PODTEKST_DATA_DIR", options.data_dir)
        .env("MEETING_RECORDER_DATA_DIR", options.data_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (key, value) in &options.variant.env {
        command.env(key, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            result.error = format!("{}: {error}", options.app.display());
            return result;
        }
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > options.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                result.error = "timeout".to_string();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                result.error = error.to_string();
                break None;
            }
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let Some(status) = status else {
        return result;
    };
    result.exit_code = status.code().unwrap_or(1);
    if !status.success() {
        result.error = stderr
            .lines()
            .last()
            .unwrap_or("transcription failed")
            .to_string();
        return result;
    }
    let transcript = stem.with_extension("transcript.json");
    let document: Value = match std::fs::read(&transcript)
        .map_err(|error| format!("{}: {error}", transcript.display()))
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
    {
        Ok(document) => document,
        Err(error) => {
            result.error = error;
            return result;
        }
    };
    result.exit_code = 0;
    result.ctc_used = Some(document["meta"]["confidence"].is_object());
    result.hotwords_applied = document["meta"]["hotwords_applied"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    result.hypothesis = document["utterances"]
        .as_array()
        .map(|utterances| {
            utterances
                .iter()
                .filter_map(|utterance| utterance["text"].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    if result.hypothesis.is_empty() {
        result.error = "empty transcript".to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_and_swept_variants() {
        let vocabulary = vec!["альфа".to_string()];
        assert!(Variant::parse("off", &vocabulary)
            .unwrap()
            .hotwords
            .is_empty());
        assert_eq!(
            Variant::parse("on", &vocabulary).unwrap().hotwords,
            vocabulary
        );
        assert!(!Variant::parse("ctc-off", &vocabulary).unwrap().ctc_enabled);
        assert_eq!(
            Variant::parse("on-b2.5", &vocabulary).unwrap().env[1].1,
            "2.5"
        );
        assert_eq!(
            Variant::parse("on-c0.4", &vocabulary).unwrap().env[1].1,
            "0.4"
        );
        assert!(Variant::parse("on-b11", &vocabulary).is_err());
        assert!(Variant::parse("on-c2", &vocabulary).is_err());
        assert!(Variant::parse("nope", &vocabulary).is_err());
    }

    #[test]
    fn config_merges_and_restores() {
        let directory = std::env::temp_dir().join(format!("mr-eval-config-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.json");
        std::fs::write(&path, b"{\"record_mode\":\"split\"}").unwrap();
        {
            let _guard = ConfigGuard::apply(&directory, &["альфа".to_string()], true).unwrap();
            let config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(config["record_mode"], "split");
            assert_eq!(config["ctc_enabled"], true);
        }
        let restored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored["record_mode"], "split");
        assert!(restored.get("hotwords").is_none());
        let _ = std::fs::remove_dir_all(directory);
    }
}
