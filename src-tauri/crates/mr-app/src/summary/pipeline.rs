use std::time::Instant;

use mr_summarize::artifact::{
    ArtifactClassification, Generator, PresetInfo, Provenance, SourceInfo, SummaryArtifact,
    SummaryBody, Timings, Warning,
};
use mr_summarize::{classify, parse, preset, prompt, text, Detail, SummaryError, Transcript};

use super::backend::{
    BackendError, BackendEvent, BackendInfo, CancelToken, GenerateRequest, Phase, SummaryBackend,
};
use super::config::{CONTEXT_TOKENS, MODEL_ID, MODEL_TITLE, MODEL_TITLE_EN, RUNTIME_COMMIT};
use super::store::Paths;

pub struct JobSpec {
    pub paths: Paths,
    pub preset_id: String,
    pub detail: Detail,
    pub confirmed_sensitive: bool,
    pub allow_downgrade: bool,
    pub force: bool,
    pub model_sha: String,
}

pub struct PipelineOutput {
    pub artifact: SummaryArtifact,
    pub md_written: bool,
}

fn map_backend(error: BackendError) -> super::SummaryJobError {
    match error {
        BackendError::Cancelled => super::SummaryJobError::Cancelled,
        BackendError::Load(_) => super::SummaryJobError::ModelLoad,
        BackendError::Oom => super::SummaryJobError::Oom,
        BackendError::Crashed => super::SummaryJobError::BackendCrashed,
        BackendError::Http(message) => super::SummaryJobError::Internal(message),
    }
}

fn estimate_single_pass(chars: usize, max_tokens: u32) -> bool {
    let prompt_tokens = chars as f64 / 2.5 + 600.0;
    chars <= 12_000
        && prompt_tokens <= 6_000.0
        && prompt_tokens as u32 + max_tokens + 256 <= CONTEXT_TOKENS
}

pub fn run(
    spec: &JobSpec,
    backend: &mut dyn SummaryBackend,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(BackendEvent),
) -> Result<PipelineOutput, super::SummaryJobError> {
    let started = Instant::now();
    let source = std::fs::read_to_string(&spec.paths.transcript)
        .map_err(|_| super::SummaryJobError::NoTranscript)?;
    let transcript = Transcript::from_json(&source).map_err(|_| super::SummaryJobError::NoText)?;
    let check = classify::validate_source(&transcript).map_err(|e| match e {
        SummaryError::TooShort { .. } => super::SummaryJobError::TooShort,
        _ => super::SummaryJobError::NoText,
    })?;
    let preset =
        preset::require(&spec.preset_id).map_err(|_| super::SummaryJobError::InvalidPreset)?;
    let rev = mr_summarize::artifact::source_rev(&transcript.utterances);
    let language = mr_summarize::text::language_of(&transcript.full_text());
    let output_language = if language == text::Language::En {
        text::Language::En
    } else {
        text::Language::Ru
    };
    let classification = classify::classify(&transcript);
    let info = backend.probe().map_err(map_backend)?;
    cancel.check().map_err(map_backend)?;

    on_event(BackendEvent {
        phase: Phase::Load,
        tokens_done: 0,
        tokens_total: 0,
    });
    let prepare_started = Instant::now();
    on_event(BackendEvent {
        phase: Phase::Prepare,
        tokens_done: 0,
        tokens_total: 0,
    });
    let chars = transcript.total_chars();
    let max_tokens = spec.detail.max_tokens();
    let single = estimate_single_pass(chars, max_tokens);
    let mut warnings: Vec<Warning> = Vec::new();
    if check.short {
        warnings.push(Warning::ShortSource);
    }
    if super::memory::logical_cores() < 4 {
        warnings.push(Warning::SlowHardware);
    }
    if language == text::Language::Other {
        warnings.push(Warning::LanguageMixed);
    }
    if spec.detail == Detail::Long && chars <= 12_000 {
        warnings.push(Warning::DetailLongShortSource);
    }

    let generate_started = Instant::now();
    let prepare_ms = prepare_started.elapsed().as_millis() as u64;
    let mut retry = RetryState::default();
    let mut truncated = false;
    let mut coverage = 1.0f64;
    let mut chunk_count = 1usize;
    let mut reduce_levels = 0usize;
    let mut ctx = GenCtx {
        info: &info,
        max_tokens,
        cancel,
        on_event: &mut *on_event,
        retry: &mut retry,
        language,
    };
    let mut map_warnings: Vec<Warning> = Vec::new();
    let mut map_injection = 0usize;
    let (parsed, last_req, last_nonce) = if single {
        let prompt = prompt::build(preset, &transcript.utterances, language);
        let req = request(&info, &prompt, max_tokens);
        let generated = generate_with_retry(backend, &req, preset, &mut ctx, &prompt.nonce)?;
        (generated.parsed, generated.req, generated.nonce)
    } else {
        let plan = mr_summarize::chunk::plan(&transcript, mr_summarize::chunk::MAX_CHUNKS);
        truncated = plan.truncated;
        coverage = plan.coverage;
        chunk_count = plan.chunks.len();
        if plan.is_empty() {
            return Err(super::SummaryJobError::InvalidOutput);
        }
        let total = plan.chunks.len() as u32;
        let mut merged = parse::empty(preset);
        let mut partials: Vec<String> = Vec::new();
        let mut empty_chunks = 0usize;
        for (index, chunk) in plan.chunks.iter().enumerate() {
            cancel.check().map_err(map_backend)?;
            (ctx.on_event)(BackendEvent {
                phase: Phase::Map,
                tokens_done: index as u32,
                tokens_total: total,
            });
            let slice = &transcript.utterances[chunk.start..chunk.end];
            let prompt = prompt::build(preset, slice, language);
            let req = request(&info, &prompt, max_tokens);
            let generated = generate_with_retry(backend, &req, preset, &mut ctx, &prompt.nonce)?;
            let items = generated
                .parsed
                .sections
                .iter()
                .map(|s| s.items.len())
                .sum::<usize>();
            if items == 0 {
                empty_chunks += 1;
            }
            partials.push(generated.raw);
            for warning in &generated.parsed.warnings {
                if !map_warnings.contains(warning) {
                    map_warnings.push(*warning);
                }
            }
            map_injection += generated.parsed.injection_flagged;
            for (target, source) in merged.sections.iter_mut().zip(generated.parsed.sections) {
                target.items.extend(source.items);
            }
            merged.parser_recovered += generated.parsed.parser_recovered;
        }
        if empty_chunks * 10 > plan.chunks.len() * 3 {
            warnings.push(Warning::CoverageLow);
        }
        if merged.sections.iter().all(|s| s.items.is_empty()) {
            return Err(super::SummaryJobError::InvalidOutput);
        }
        (ctx.on_event)(BackendEvent {
            phase: Phase::Reduce,
            tokens_done: 0,
            tokens_total: 0,
        });
        let repair_prompt = prompt::build_reduce(preset, language, &partials);
        let fallback_nonce = repair_prompt.nonce.clone();
        let repair_req = request(&info, &repair_prompt, max_tokens);
        let reduced = if partials.len() < 2 {
            None
        } else {
            reduce_all(
                backend,
                preset,
                language,
                &partials,
                &mut ctx,
                &mut reduce_levels,
            )?
        };
        match reduced {
            Some((parsed, req, nonce)) => (parsed, req, nonce),
            None => {
                if partials.len() >= 2 {
                    // Reduce не дал валидных секций: не теряем job, отдаём
                    // заземлённый merge частичных ответов (§9).
                    merged.parser_recovered += 1;
                    warnings.push(Warning::ParserRecovered);
                    ctx.retry.reason = Some("reduce_invalid".to_string());
                }
                (merged, repair_req, fallback_nonce)
            }
        }
    };
    warnings.extend(map_warnings);
    if map_injection > 0 && !warnings.contains(&Warning::InjectionSuspected) {
        warnings.push(Warning::InjectionSuspected);
    }

    (ctx.on_event)(BackendEvent {
        phase: Phase::Verify,
        tokens_done: 0,
        tokens_total: 0,
    });
    let mut verify_ms = 0u64;
    let started_grounding = Instant::now();
    let mut grounded =
        mr_summarize::grounding::ground(&parsed, &transcript, preset, spec.detail, output_language);
    verify_ms += started_grounding.elapsed().as_millis() as u64;
    let mut items_total: usize = grounded.sections.iter().map(|s| s.items.len()).sum();
    let decision_cue = mr_summarize::grounding::has_decision_cue(&transcript);
    let decisions_empty = |grounded: &mr_summarize::grounding::GroundedSummary| {
        !grounded
            .sections
            .iter()
            .any(|s| s.id == "decisions" && !s.items.is_empty())
    };
    let thin = items_total == 0 || (chars >= 1000 && items_total < 2);
    let decision_missed = decision_cue && decisions_empty(&grounded);
    if thin || decision_missed {
        let mut reasons: Vec<&str> = Vec::new();
        if thin {
            reasons.push("output_thin");
        }
        if decision_missed {
            reasons.push("decision_missed");
        }
        ctx.retry.retries += 1;
        ctx.retry.reason = Some(reasons.join(","));
        let repair = repair_request(&last_req, thin, decision_missed);
        match backend.generate(&repair, ctx.cancel, &mut *ctx.on_event) {
            Ok(out) => {
                let (out, injected) = screen_output(out, &last_nonce);
                match parse::parse(&out, preset) {
                    Ok(mut parsed2) => {
                        if injected {
                            mark_injection(&mut parsed2);
                        }
                        let retry_grounding = Instant::now();
                        let grounded2 = mr_summarize::grounding::ground(
                            &parsed2,
                            &transcript,
                            preset,
                            spec.detail,
                            output_language,
                        );
                        verify_ms += retry_grounding.elapsed().as_millis() as u64;
                        let items2: usize = grounded2.sections.iter().map(|s| s.items.len()).sum();
                        let thin2 = items2 == 0 || (chars >= 1000 && items2 < 2);
                        if thin2 {
                            if thin {
                                return Err(super::SummaryJobError::OutputThin);
                            }
                        } else {
                            grounded = grounded2;
                            items_total = items2;
                        }
                    }
                    Err(_) => {
                        if thin {
                            return Err(super::SummaryJobError::OutputThin);
                        }
                    }
                }
            }
            Err(BackendError::Cancelled) => {
                return Err(super::SummaryJobError::Cancelled);
            }
            Err(error) => {
                if thin {
                    return Err(map_backend(error));
                }
            }
        }
    }
    if items_total == 0 || (chars >= 1000 && items_total < 2) {
        return Err(super::SummaryJobError::OutputThin);
    }
    if decision_cue && decisions_empty(&grounded) {
        warnings.push(Warning::DecisionMissed);
    }
    let load_ms = backend.last_load_ms();
    let generation_elapsed = generate_started.elapsed().as_millis() as u64;
    let generate_ms = generation_elapsed.saturating_sub(verify_ms.saturating_add(load_ms));
    warnings.extend(grounded.warnings.iter().cloned());
    if truncated {
        warnings.push(Warning::Truncated);
    }
    if coverage < 0.99 && !truncated {
        warnings.push(Warning::CoverageLow);
    }
    warnings.sort_by_key(|w| format!("{w:?}"));
    warnings.dedup();

    let total_ms = started.elapsed().as_millis() as u64;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut artifact = SummaryArtifact {
        schema: mr_summarize::artifact::SCHEMA.to_string(),
        schema_minor: mr_summarize::artifact::SCHEMA_MINOR,
        id: super::id::new_job_id(),
        generated_at: now.clone(),
        source: SourceInfo {
            schema: 1,
            rev,
            utterances: transcript.non_empty_texts().count(),
            chars,
            language,
            duration_sec: duration(&transcript),
        },
        classification: ArtifactClassification::from_classification(
            &classification,
            now,
            classification.preset_id.as_deref() != Some(spec.preset_id.as_str()),
        ),
        preset: PresetInfo::from_preset(preset, spec.confirmed_sensitive, output_language),
        detail: spec.detail,
        output_language,
        generator: Generator {
            runtime: info.runtime.clone(),
            runtime_commit: if info.runtime == "fake" {
                "fake".to_string()
            } else {
                RUNTIME_COMMIT.to_string()
            },
            backend: "sidecar".to_string(),
            model_id: MODEL_ID.to_string(),
            model_title: if output_language == text::Language::En {
                MODEL_TITLE_EN.to_string()
            } else {
                MODEL_TITLE.to_string()
            },
            model_sha256: spec.model_sha.clone(),
            quant: "Q4_K_M".to_string(),
            context: CONTEXT_TOKENS,
            threads: info.threads,
            temperature: 0.0,
            top_p: 0.9,
            repeat_penalty: 1.1,
            max_tokens,
            seed: 42,
            prompt_version: preset.prompt_version.to_string(),
            duration_ms: total_ms,
            timings_ms: Timings {
                load: load_ms,
                prepare: prepare_ms,
                generate: generate_ms,
                verify: verify_ms,
                total: total_ms,
            },
        },
        summary: SummaryBody {
            sections: grounded.sections,
            warnings,
            truncated,
            coverage,
            items_total,
        },
        provenance: Provenance {
            mode: if single { "single_pass" } else { "map_reduce" }.to_string(),
            chunks: chunk_count,
            reduce_levels,
            items_dropped: grounded.items_dropped,
            soft_claims_dropped: grounded.soft_claims_dropped,
            parser_retries: retry.retries,
            parser_recovered: grounded.parser_recovered,
            retry_reason: retry.reason,
        },
    };
    artifact
        .enforce_caps()
        .map_err(|_| super::SummaryJobError::OutputCapped)?;
    // Commit point: после этой проверки cancel игнорируется до terminal.
    cancel.check().map_err(map_backend)?;
    // Расшифровка могла измениться, пока модель работала.
    let current = std::fs::read_to_string(&spec.paths.transcript)
        .ok()
        .and_then(|text| Transcript::from_json(&text).ok())
        .map(|t| mr_summarize::artifact::source_rev(&t.utterances));
    if current.as_deref() != Some(artifact.source.rev.as_str()) {
        return Err(super::SummaryJobError::StaleSource);
    }
    match super::store::inspect(&spec.paths) {
        super::store::State::Corrupt => super::store::backup_corrupt(&spec.paths)
            .map_err(|e| super::SummaryJobError::Internal(e.code().to_string()))?,
        super::store::State::Unsupported if spec.allow_downgrade && spec.force => {
            super::store::backup_newer(&spec.paths)
                .map_err(|e| super::SummaryJobError::Internal(e.code().to_string()))?
        }
        super::store::State::Unsupported => return Err(super::SummaryJobError::SchemaUnsupported),
        _ => {}
    }
    let md_written = match super::store::write(&spec.paths, &artifact) {
        Ok(written) => written,
        Err(super::store::StoreError::OutputCapped) => {
            return Err(super::SummaryJobError::OutputCapped)
        }
        Err(e) => return Err(super::SummaryJobError::Internal(e.code().to_string())),
    };
    if !md_written {
        artifact.summary.warnings.push(Warning::MdWriteFailed);
        // JSON уже валиден; повторная запись сохраняет warning для reopen.
        if let Ok(json) = artifact.to_json() {
            let _ = crate::fsutil::write_atomic_private(&spec.paths.json, json.as_bytes());
        }
    }
    Ok(PipelineOutput {
        artifact,
        md_written,
    })
}

#[derive(Default)]
struct RetryState {
    retries: usize,
    reason: Option<String>,
}

struct GenCtx<'a> {
    info: &'a BackendInfo,
    max_tokens: u32,
    cancel: &'a CancelToken,
    on_event: &'a mut dyn FnMut(BackendEvent),
    retry: &'a mut RetryState,
    language: text::Language,
}

struct Generated {
    raw: String,
    parsed: mr_summarize::parse::ParsedSummary,
    req: GenerateRequest,
    nonce: String,
}

fn screen_output(mut out: String, nonce: &str) -> (String, bool) {
    if !prompt::nonce_in_output(&out, nonce) {
        return (out, false);
    }
    out = out
        .lines()
        .filter(|line| !line.contains(nonce))
        .collect::<Vec<_>>()
        .join("\n");
    (out, true)
}

fn mark_injection(parsed: &mut mr_summarize::parse::ParsedSummary) {
    parsed.injection_flagged += 1;
    if !parsed.warnings.contains(&Warning::InjectionSuspected) {
        parsed.warnings.push(Warning::InjectionSuspected);
    }
}

fn generate_with_retry(
    backend: &mut dyn SummaryBackend,
    req: &GenerateRequest,
    preset: &preset::Preset,
    ctx: &mut GenCtx<'_>,
    nonce: &str,
) -> Result<Generated, super::SummaryJobError> {
    let (out, injected) = screen_output(
        backend
            .generate(req, ctx.cancel, ctx.on_event)
            .map_err(map_backend)?,
        nonce,
    );
    // Плоский ответ с пунктами parser восстанавливает сам; repair нужен
    // только когда пунктов нет (иначе лишний вызов 60–140 с).
    match parse::parse(&out, preset) {
        Ok(mut parsed) => {
            if injected {
                mark_injection(&mut parsed);
            }
            Ok(Generated {
                raw: out,
                parsed,
                req: req.clone(),
                nonce: nonce.to_string(),
            })
        }
        Err(_) => {
            ctx.retry.retries += 1;
            ctx.retry.reason = Some("invalid_output".to_string());
            let repair_prompt = prompt::build_repair(preset, ctx.language, &out);
            let retry = request_seed(&request(ctx.info, &repair_prompt, ctx.max_tokens), 43);
            let (repaired, injected) = screen_output(
                backend
                    .generate(&retry, ctx.cancel, ctx.on_event)
                    .map_err(map_backend)?,
                &repair_prompt.nonce,
            );
            let mut parsed = parse::parse(&repaired, preset)
                .map_err(|_| super::SummaryJobError::InvalidOutput)?;
            if injected {
                mark_injection(&mut parsed);
            }
            Ok(Generated {
                raw: repaired,
                parsed,
                req: retry,
                nonce: repair_prompt.nonce,
            })
        }
    }
}

/// Один или два уровня reduce. `None` — reduce не дал валидных секций;
/// вызывающий обязан отдать заземлённый merge частичных ответов.
fn reduce_all(
    backend: &mut dyn SummaryBackend,
    preset: &preset::Preset,
    language: text::Language,
    partials: &[String],
    ctx: &mut GenCtx<'_>,
    reduce_levels: &mut usize,
) -> Result<
    Option<(mr_summarize::parse::ParsedSummary, GenerateRequest, String)>,
    super::SummaryJobError,
> {
    if partials.len() < 2 {
        return Ok(None);
    }
    let mut inputs = partials.to_vec();
    loop {
        let groups: Vec<Vec<String>> = if inputs.len() > 10 {
            inputs.chunks(10).map(|chunk| chunk.to_vec()).collect()
        } else {
            vec![inputs.clone()]
        };
        let mut next: Vec<String> = Vec::new();
        let mut final_gen: Option<Generated> = None;
        for group in groups {
            if group.len() == 1 {
                next.push(group[0].clone());
                continue;
            }
            let prompt = prompt::build_reduce(preset, language, &group);
            let req = request(ctx.info, &prompt, ctx.max_tokens);
            match generate_with_retry(backend, &req, preset, ctx, &prompt.nonce) {
                Ok(generated) => {
                    next.push(generated.raw.clone());
                    final_gen = Some(generated);
                }
                Err(super::SummaryJobError::InvalidOutput) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        *reduce_levels += 1;
        if next.len() == 1 {
            let generated = final_gen.ok_or_else(|| {
                super::SummaryJobError::Internal("reduce without output".to_string())
            })?;
            return Ok(Some((generated.parsed, generated.req, generated.nonce)));
        }
        inputs = next;
        if *reduce_levels >= 2 {
            return Ok(None);
        }
    }
}

fn repair_request(req: &GenerateRequest, thin: bool, decision_missed: bool) -> GenerateRequest {
    let mut retry = request_seed(req, 43);
    if thin {
        retry.prompt.push_str(
            "\n\nПредыдущий ответ был слишком коротким. Верни минимум два содержательных пункта, если в источнике есть факты; не добавляй факты.",
        );
    }
    if decision_missed {
        retry.prompt.push_str(
            "\n\nВ источнике есть явное решение. Обязательно заполни секцию решений дословным решением из источника или не выводи её, если cue нет.",
        );
    }
    retry
}

fn request(
    info: &BackendInfo,
    prompt: &mr_summarize::prompt::Prompt,
    max_tokens: u32,
) -> GenerateRequest {
    GenerateRequest {
        model_path: super::model_path().unwrap_or_default(),
        system: prompt.system.clone(),
        prompt: prompt.user.clone(),
        max_tokens,
        temperature: 0.0,
        top_p: 0.9,
        repeat_penalty: 1.1,
        seed: 42,
        n_ctx: CONTEXT_TOKENS,
        threads: info.threads,
        stop: vec!["<|im_end|>".to_string(), "<|endoftext|>".to_string()],
    }
}

fn request_seed(req: &GenerateRequest, seed: u64) -> GenerateRequest {
    let mut retry = req.clone();
    retry.seed = seed;
    retry
}

fn duration(transcript: &Transcript) -> f64 {
    let end = transcript.utterances.iter().map(|u| u.end).reduce(f64::max);
    let start = transcript
        .utterances
        .iter()
        .map(|u| u.start)
        .reduce(f64::min);
    match (end, start) {
        (Some(end), Some(start)) => (end - start).max(0.0),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_minute_source_stays_single_pass() {
        assert!(estimate_single_pass(9_500, 768));
    }

    #[test]
    fn fake_flat_output_is_recovered_without_repair() {
        let dir = std::env::temp_dir().join(format!(
            "mr-pipeline-flat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let utterances = vec![
            serde_json::json!({"start":0.0,"end":1.0,"speaker":"Анна","text":"Решили перенести запуск."}),
            serde_json::json!({"start":1.0,"end":2.0,"speaker":"Борис","text":"Борис доделает отчёт до среды."}),
            serde_json::json!({"start":2.0,"end":3.0,"speaker":"Анна","text":"Что осталось по бюджету?"}),
        ];
        let paths = super::super::store::summary_paths(&dir, "flat", "flat.wav".into());
        std::fs::write(
            &paths.transcript,
            serde_json::to_vec(&serde_json::json!({"utterances": utterances})).unwrap(),
        )
        .unwrap();
        let spec = JobSpec {
            paths,
            preset_id: "meeting".to_string(),
            detail: Detail::Normal,
            confirmed_sensitive: false,
            allow_downgrade: false,
            force: false,
            model_sha: super::super::config::MODEL_SHA256.to_string(),
        };
        let mut backend = super::super::fake::FakeBackend::new("flat_once");
        let cancel = CancelToken::new();
        let mut on_event = |_event: BackendEvent| {};
        let output = super::run(&spec, &mut backend, &cancel, &mut on_event)
            .expect("flat parser должен дать артефакт без repair");
        assert_eq!(backend.calls(), 1, "лишний repair-вызов для flat output");
        assert_eq!(output.artifact.provenance.retry_reason, None);
        let sections = &output.artifact.summary.sections;
        assert!(sections
            .iter()
            .any(|s| s.id == "decisions" && !s.items.is_empty()));
        let task = sections
            .iter()
            .find(|s| s.id == "tasks")
            .and_then(|s| s.items.first())
            .and_then(|i| i.task.as_ref())
            .expect("task потерян");
        assert_eq!(task.assignee.as_deref(), Some("Борис"));
        assert_eq!(task.deadline.as_deref(), Some("до среды"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_source_goes_map_reduce() {
        assert!(!estimate_single_pass(120_000, 768));
    }

    #[test]
    fn fake_long_source_uses_reduce_and_grounds() {
        let dir = std::env::temp_dir().join(format!(
            "mr-pipeline-reduce-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let utterances: Vec<serde_json::Value> = (0..200)
            .map(|i| {
                serde_json::json!({
                    "start": i as f64,
                    "end": i as f64 + 0.9,
                    "speaker": "Спикер 1",
                    "text": format!("Обсудили пункт {i}: решили подготовить отчёт и проверить срок до среды."),
                })
            })
            .collect();
        let transcript = serde_json::json!({"utterances": utterances});
        let paths = super::super::store::summary_paths(&dir, "long", "long.wav".into());
        std::fs::write(&paths.transcript, serde_json::to_vec(&transcript).unwrap()).unwrap();
        let spec = JobSpec {
            paths,
            preset_id: "meeting".to_string(),
            detail: Detail::Normal,
            confirmed_sensitive: false,
            allow_downgrade: false,
            force: false,
            model_sha: super::super::config::MODEL_SHA256.to_string(),
        };
        let mut backend = super::super::fake::FakeBackend::new("ok");
        let cancel = CancelToken::new();
        let mut on_event = |_event: BackendEvent| {};
        let output = super::run(&spec, &mut backend, &cancel, &mut on_event)
            .expect("fake map-reduce должен завершиться");
        assert_eq!(output.artifact.provenance.mode, "map_reduce");
        assert_eq!(output.artifact.provenance.reduce_levels, 1);
        assert!(!output.artifact.summary.sections.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn english_fixture_keeps_titles_deadline_and_locale() {
        let dir = std::env::temp_dir().join(format!(
            "mr-pipeline-en-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let utterances = vec![
            serde_json::json!({"start":0.0,"end":1.0,"speaker":"Ann","text":"We decided to move the launch to next week."}),
            serde_json::json!({"start":1.0,"end":2.0,"speaker":"Bob","text":"Bob will update the report by Friday."}),
            serde_json::json!({"start":2.0,"end":3.0,"speaker":"Ann","text":"Who checks the budget?"}),
        ];
        let paths = super::super::store::summary_paths(&dir, "en", "en.wav".into());
        std::fs::write(
            &paths.transcript,
            serde_json::to_vec(&serde_json::json!({"utterances": utterances})).unwrap(),
        )
        .unwrap();
        let spec = JobSpec {
            paths,
            preset_id: "meeting".to_string(),
            detail: Detail::Normal,
            confirmed_sensitive: false,
            allow_downgrade: false,
            force: false,
            model_sha: super::super::config::MODEL_SHA256.to_string(),
        };
        let mut backend = super::super::fake::FakeBackend::new("ok");
        let cancel = CancelToken::new();
        let mut on_event = |_event: BackendEvent| {};
        let output = super::run(&spec, &mut backend, &cancel, &mut on_event)
            .expect("english fixture должен собраться");
        let artifact = output.artifact;
        assert_eq!(artifact.output_language, mr_summarize::text::Language::En);
        assert_eq!(artifact.source.language, mr_summarize::text::Language::En);
        assert!(artifact
            .summary
            .sections
            .iter()
            .all(|s| s.title != "Кратко" && s.title != "Задачи" && s.title != "Решения"));
        let tasks = artifact
            .summary
            .sections
            .iter()
            .find(|s| s.id == "tasks")
            .expect("tasks потеряны");
        assert_eq!(tasks.title, "Tasks");
        let task = tasks
            .items
            .iter()
            .find(|i| i.task.is_some())
            .and_then(|i| i.task.as_ref())
            .expect("task без метаданных");
        assert_eq!(task.deadline.as_deref(), Some("by Friday"));
        assert_eq!(task.assignee.as_deref(), Some("Bob"));
        assert_eq!(artifact.preset.label, "Meeting");
        assert_eq!(
            artifact.generator.model_title,
            "Qwen3-4B Instruct 2507 (summary, Q4_K_M)"
        );
        let markdown = artifact.to_markdown();
        assert!(markdown.contains("# Meeting summary"), "{markdown}");
        assert!(markdown.contains("## Tasks"), "{markdown}");
        assert!(markdown.contains("Preset: «Meeting»"), "{markdown}");
        assert!(markdown.contains("Who: Bob; Due: by Friday"), "{markdown}");
        assert!(!markdown.contains("## Кратко"), "{markdown}");
        assert!(!markdown.contains("Кто:"), "{markdown}");
        assert!(
            !markdown
                .chars()
                .any(|c| ('\u{0400}'..='\u{052F}').contains(&c)),
            "русский текст в английском Markdown: {markdown}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn echoed_nonce_is_dropped_and_flagged() {
        let dir = std::env::temp_dir().join(format!(
            "mr-pipeline-nonce-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let utterances = vec![
            serde_json::json!({"start":0.0,"end":1.0,"speaker":"Анна","text":"Нормальный пункт из источника."}),
            serde_json::json!({"start":1.0,"end":2.0,"speaker":"Борис","text":"Второй нормальный пункт."}),
        ];
        let paths = super::super::store::summary_paths(&dir, "nonce", "nonce.wav".into());
        std::fs::write(
            &paths.transcript,
            serde_json::to_vec(&serde_json::json!({"utterances": utterances})).unwrap(),
        )
        .unwrap();
        let spec = JobSpec {
            paths,
            preset_id: "meeting".to_string(),
            detail: Detail::Normal,
            confirmed_sensitive: false,
            allow_downgrade: false,
            force: false,
            model_sha: super::super::config::MODEL_SHA256.to_string(),
        };
        let mut backend = super::super::fake::FakeBackend::new("nonce_echo");
        let cancel = CancelToken::new();
        let mut on_event = |_event: BackendEvent| {};
        let output = super::run(&spec, &mut backend, &cancel, &mut on_event)
            .expect("nonce не должен ронять весь job");
        assert!(output
            .artifact
            .summary
            .warnings
            .contains(&Warning::InjectionSuspected));
        assert!(output
            .artifact
            .summary
            .sections
            .iter()
            .flat_map(|s| &s.items)
            .all(|i| !i.text.contains("Утечка")));
        let json = output.artifact.to_json().unwrap();
        assert!(!json.contains("Утечка"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn map_chunk_injection_reaches_final_warnings() {
        let dir = std::env::temp_dir().join(format!(
            "mr-pipeline-map-inj-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let utterances: Vec<serde_json::Value> = (0..200)
            .map(|i| {
                serde_json::json!({
                    "start": i as f64,
                    "end": i as f64 + 0.9,
                    "speaker": "Спикер 1",
                    "text": format!("Обсудили пункт {i}: решили подготовить отчёт и проверить срок до среды."),
                })
            })
            .collect();
        let paths = super::super::store::summary_paths(&dir, "mapinj", "mapinj.wav".into());
        std::fs::write(
            &paths.transcript,
            serde_json::to_vec(&serde_json::json!({"utterances": utterances})).unwrap(),
        )
        .unwrap();
        let spec = JobSpec {
            paths,
            preset_id: "meeting".to_string(),
            detail: Detail::Normal,
            confirmed_sensitive: false,
            allow_downgrade: false,
            force: false,
            model_sha: super::super::config::MODEL_SHA256.to_string(),
        };
        let mut backend = super::super::fake::FakeBackend::new("map_injection");
        let cancel = CancelToken::new();
        let mut on_event = |_event: BackendEvent| {};
        let output = super::run(&spec, &mut backend, &cancel, &mut on_event)
            .expect("map injection не должен терять job");
        assert_eq!(output.artifact.provenance.mode, "map_reduce");
        assert!(
            output
                .artifact
                .summary
                .warnings
                .contains(&Warning::InjectionSuspected),
            "map-инъекция потерялась: {:?}",
            output.artifact.summary.warnings
        );
        assert!(output
            .artifact
            .summary
            .sections
            .iter()
            .flat_map(|s| &s.items)
            .all(|i| !i.text.to_lowercase().contains("ignore previous")));
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn model_load_failure_writes_no_artifact() {
        let dir = std::env::temp_dir().join(format!(
            "mr-pipeline-load-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let utterances = vec![
            serde_json::json!({"start":0.0,"end":1.0,"speaker":"Анна","text":"Решили перенести запуск."}),
            serde_json::json!({"start":1.0,"end":2.0,"speaker":"Борис","text":"Надо подготовить отчёт до среды."}),
        ];
        let paths = super::super::store::summary_paths(&dir, "load", "load.wav".into());
        std::fs::write(
            &paths.transcript,
            serde_json::to_vec(&serde_json::json!({"utterances": utterances})).unwrap(),
        )
        .unwrap();
        let spec = JobSpec {
            paths: paths.clone(),
            preset_id: "meeting".to_string(),
            detail: Detail::Normal,
            confirmed_sensitive: false,
            allow_downgrade: false,
            force: false,
            model_sha: super::super::config::MODEL_SHA256.to_string(),
        };
        let mut backend = super::super::fake::FakeBackend::new("model_load");
        let cancel = CancelToken::new();
        let mut on_event = |_event: BackendEvent| {};
        let error = match super::run(&spec, &mut backend, &cancel, &mut on_event) {
            Ok(_) => panic!("model_load не вернул ошибку"),
            Err(error) => error,
        };
        assert_eq!(error, super::super::SummaryJobError::ModelLoad);
        assert!(!paths.json.exists(), "артефакт записан после model_load");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
