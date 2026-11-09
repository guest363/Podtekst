use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use mr_summarize::{classify, Classification, Transcript};

use super::store::{Paths, StoreError};

#[derive(Clone)]
pub struct Source {
    pub transcript: Transcript,
    pub rev: String,
}

type SourceKey = String;

fn source_cache() -> &'static Mutex<HashMap<SourceKey, Source>> {
    static CACHE: OnceLock<Mutex<HashMap<SourceKey, Source>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn source_order() -> &'static Mutex<VecDeque<SourceKey>> {
    static ORDER: OnceLock<Mutex<VecDeque<SourceKey>>> = OnceLock::new();
    ORDER.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Ключ `(path,len,mtime)`: правка расшифровки меняет mtime и промахивается.
fn source_key(paths: &Paths) -> SourceKey {
    let meta = std::fs::metadata(&paths.transcript).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}|{len}|{mtime}", paths.transcript.display())
}

pub fn read(paths: &Paths) -> Result<Source, StoreError> {
    let key = source_key(paths);
    if let Some(hit) = source_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        return Ok(hit.clone());
    }
    let text = std::fs::read_to_string(&paths.transcript).map_err(|_| StoreError::NoTranscript)?;
    let transcript = Transcript::from_json(&text).map_err(|_| StoreError::NoText)?;
    match classify::validate_source(&transcript) {
        Ok(_) => {}
        Err(mr_summarize::SummaryError::TooShort { .. }) => return Err(StoreError::TooShort),
        Err(_) => return Err(StoreError::NoText),
    }
    let rev = mr_summarize::artifact::source_rev(&transcript.utterances);
    let source = Source { transcript, rev };
    {
        let mut cache = source_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(key.clone(), source.clone());
    }
    let mut order = source_order().lock().unwrap_or_else(|e| e.into_inner());
    order.push_back(key);
    while order.len() > 8 {
        if let Some(old) = order.pop_front() {
            source_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&old);
        }
    }
    Ok(source)
}

type ClassKey = (String, u32, String);

fn class_cache() -> &'static Mutex<HashMap<ClassKey, Classification>> {
    static CACHE: OnceLock<Mutex<HashMap<ClassKey, Classification>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn class_order() -> &'static Mutex<VecDeque<ClassKey>> {
    static ORDER: OnceLock<Mutex<VecDeque<ClassKey>>> = OnceLock::new();
    ORDER.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// LRU 64 по `(source_rev, heuristic_version, spec_version)`; не персистится.
pub fn classify_cached(source: &Source) -> (Classification, bool) {
    let key = (
        source.rev.clone(),
        classify::HEURISTIC_VERSION,
        classify::SPEC_VERSION.to_string(),
    );
    if let Some(hit) = class_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
    {
        return (hit.clone(), true);
    }
    let result = classify::classify(&source.transcript);
    class_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone(), result.clone());
    let mut order = class_order().lock().unwrap_or_else(|e| e.into_inner());
    order.push_back(key);
    while order.len() > 64 {
        if let Some(old) = order.pop_front() {
            class_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&old);
        }
    }
    (result, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_summarize::Utterance;

    #[test]
    fn classification_key_includes_revision() {
        let source = Source {
            transcript: Transcript {
                utterances: vec![Utterance {
                    start: 0.0,
                    end: 1.0,
                    speaker: "S".into(),
                    text: "Обсудили срок запуска.".into(),
                }],
            },
            rev: "rev-a".into(),
        };
        let (a, hit) = classify_cached(&source);
        assert!(!hit);
        let (_, hit) = classify_cached(&source);
        assert!(hit);
        let other = Source {
            rev: "rev-b".into(),
            ..source
        };
        let (b, hit) = classify_cached(&other);
        assert!(!hit);
        assert_eq!(a.preset_id, b.preset_id);
    }
}
