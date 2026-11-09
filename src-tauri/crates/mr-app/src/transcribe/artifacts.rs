use std::path::Path;
use std::time::Instant;

use super::diag::{model_files, stage_begin, stage_end, AudioStats};
use super::document::{read_transcript, validate, TranscriptDoc};
use super::TranscribeOptions;

pub(super) fn write_run_manifest(
    audio_path: &Path,
    opts: &TranscribeOptions,
    duration_sec: f64,
    channels: usize,
    input: AudioStats,
    engine: &str,
) {
    if !cfg!(debug_assertions) {
        return;
    }
    let mut models = Vec::new();
    for id in [
        "vad-silero-v6",
        "asr-gigaam-v3-rnnt-int8",
        "asr-gigaam-v3-e2e-ctc-int8",
        "asr-whisper-large-v3-turbo-q",
        "diar-eres2netv2-emb",
        "diar-pyannote-segmentation-3",
    ] {
        models.extend(model_files(id));
    }
    let mut features = Vec::new();
    if cfg!(feature = "onnx") {
        features.push("onnx");
    }
    if cfg!(feature = "tauri-ui") {
        features.push("tauri-ui");
    }
    let manifest = serde_json::json!({
        "schema": 1,
        "run_id": crate::logging::current_run_id().unwrap_or_default(),
        "started_at": chrono::Local::now().to_rfc3339(),
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
            "commit": option_env!("PODTEKST_GIT_COMMIT").unwrap_or("unknown"),
            "dirty": option_env!("PODTEKST_GIT_DIRTY").unwrap_or("unknown"),
            "profile": if cfg!(debug_assertions) { "dev" } else { "release" },
            "debug_assertions": cfg!(debug_assertions),
            "features": features,
        },
        "os": {
            "name": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        },
        "input": {
            "file": audio_path.display().to_string(),
            "bytes": std::fs::metadata(audio_path).map(|m| m.len()).unwrap_or(0),
            "dur_sec": duration_sec,
            "channels": channels,
            "rate": 16000,
            "rms": input.rms,
            "peak": input.peak,
            "clip_pct": input.clip_pct,
            "silence_pct": input.silence_pct,
            "nan": input.nan,
            "inf": input.inf,
        },
        "options": {
            "engine": opts.engine.clone(),
            "language": opts.language.clone(),
            "min_speakers": opts.min_speakers,
            "max_speakers": opts.max_speakers,
            "hotwords_n": opts.hotwords.len(),
            "ctc": opts.ctc_enabled,
        },
        "engine": engine,
        "models": models,
        "repro": format!(
            "./run.sh --dev --transcribe {} --engine {}",
            audio_path.display(),
            opts.engine
        ),
    });
    crate::logging::write_manifest(&manifest);
}

pub(super) fn write_transcript_doc(path: &Path, doc: &TranscriptDoc) -> Result<(), String> {
    let json_stage = Instant::now();
    stage_begin("json");
    if cfg!(debug_assertions) {
        let meta_keys = serde_json::to_value(&doc.meta)
            .ok()
            .and_then(|value| value.as_object().map(|map| map.len()))
            .unwrap_or(0);
        log::debug!(
            target: "json",
            utts = doc.utterances.len(),
            speakers_n = doc.meta.speakers.len(),
            words = doc.stats.words,
            speech_sec = doc.stats.speech_sec,
            meta_keys = meta_keys;
            "ev=json.build"
        );
        if log::log_enabled!(target: "json", log::Level::Trace) {
            for (index, utterance) in doc.utterances.iter().enumerate() {
                log::trace!(
                    target: "json",
                    i = index,
                    start = utterance.start,
                    end = utterance.end,
                    speaker_id = crate::logging::speaker_token(&utterance.speaker),
                    ch = utterance.channel.map(|c| c as i64).unwrap_or(-1),
                    chars = utterance.text.chars().count(),
                    words = utterance.text.split_whitespace().count(),
                    diarized = utterance.diarized,
                    sil = utterance.silhouette,
                    ctc = utterance.ctc_used;
                    "ev=json.utt"
                );
            }
        }
        let violations = validate(doc);
        for violation in violations.iter().take(50) {
            log::warn!(
                target: "json",
                i = violation.index.map(|value| value as i64).unwrap_or(-1),
                field = violation.field,
                value = violation.value.clone(),
                rule = violation.rule;
                "ev=json.check"
            );
        }
        if !violations.is_empty() {
            log::error!(
                target: "json",
                path = crate::logging::file_token(path),
                violations = violations.len();
                "ev=json.invalid"
            );
            crate::logging::write_artifact_json(
                "transcript/invalid.json",
                &serde_json::json!({"violations": violations, "document": doc}),
            );
        }
    }
    let started = Instant::now();
    let json = serde_json::to_string_pretty(doc).map_err(|error| {
        log::error!(
            target: "json",
            error = error.to_string();
            "ev=json.serialize_error"
        );
        format!("Сериализация расшифровки: {error}")
    })?;
    crate::logging::write_artifact_text("transcript/pre.json", &json);
    crate::fsutil::write_atomic(path, json.as_bytes()).map_err(|e| e.to_string())?;
    log::info!(
        target: "json",
        path = crate::logging::file_token(path),
        bytes = json.len(),
        elapsed_ms = started.elapsed().as_millis() as u64;
        "ev=json.write"
    );
    if cfg!(debug_assertions) {
        match read_transcript(path) {
            Ok(back) => log::debug!(
                target: "json",
                ok = true,
                utts_in = doc.utterances.len(),
                utts_out = back.utterances.len(),
                path = crate::logging::file_token(path);
                "ev=json.roundtrip"
            ),
            Err(error) => log::warn!(
                target: "json",
                ok = false,
                utts_in = doc.utterances.len(),
                error = error.to_string(),
                path = crate::logging::file_token(path);
                "ev=json.roundtrip"
            ),
        }
        if let Ok(post) = std::fs::read_to_string(path) {
            crate::logging::write_artifact_text("transcript/post.json", &post);
        }
    }
    stage_end("json", json_stage);
    Ok(())
}
