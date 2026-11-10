use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mr_audio::decode::{decode_to_channels_16k_auto, is_video_path};
use mr_ml::hotwords::{sanitize_hotwords, DEFAULT_CONFIDENCE_THRESHOLD};

use super::artifacts::{write_run_manifest, write_transcript_doc};
use super::channel_mix;
use super::diag::{audio_stats, stage_begin, stage_end, AudioStats};
use super::document::{unique_speech_sec, Meta, Stats, TranscriptDoc};
use super::events;
use super::memory;
use super::models::ModelsGuard;
use super::report;
use super::{TranscribeOptions, TranscribeReport};

mod asr;
mod speakers;
mod vad;

const MAX_SEGMENT_SEC: f32 = 12.0;
const SEGMENT_GAP_SEC: f32 = 0.4;
const SEGMENT_PAD_SEC: f64 = 0.0;

pub(super) fn quiet_blank_penalty(duration_sec: f64, rms: f32) -> f32 {
    if duration_sec <= 4.0 && (0.0015..0.015).contains(&rms) {
        2.0
    } else {
        0.0
    }
}

pub fn transcribe_file(
    audio_path: &Path,
    opts: &TranscribeOptions,
    on_event: &dyn Fn(u8, &str),
) -> Result<TranscribeReport, String> {
    let _run_id = crate::logging::begin_run();
    let started = Instant::now();
    log::info!(
        target: "run",
        file = crate::logging::file_token(audio_path),
        engine = opts.engine.clone(),
        language = opts.language.clone(),
        min_spk = opts.min_speakers,
        max_spk = opts.max_speakers,
        hotwords_n = opts.hotwords.len(),
        ctc = opts.ctc_enabled,
        commit = option_env!("PODTEKST_GIT_COMMIT").unwrap_or("unknown"),
        dirty = option_env!("PODTEKST_GIT_DIRTY").unwrap_or("unknown");
        "ev=run.start"
    );
    let result = transcribe_file_inner(audio_path, opts, on_event);
    match &result {
        Ok(report) => {
            let json_bytes = std::fs::metadata(&report.json_path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            log::info!(
                target: "run",
                utts = report.utterances,
                words = report.words,
                speakers_n = report.speakers.len(),
                speech_sec = report.speech_sec,
                elapsed_ms = started.elapsed().as_millis() as u64,
                json_bytes = json_bytes;
                "ev=run.done"
            );
        }
        Err(error) => {
            let stage = crate::logging::current_stage();
            log::error!(
                target: "run",
                error = error.clone(),
                elapsed_ms = started.elapsed().as_millis() as u64;
                "ev=run.error"
            );
            crate::logging::append_error(&serde_json::json!({
                "stage": stage,
                "error": error,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }));
        }
    }
    crate::logging::flush();
    crate::logging::end_run();
    result
}

fn transcribe_file_inner(
    audio_path: &Path,
    opts: &TranscribeOptions,
    on_event: &dyn Fn(u8, &str),
) -> Result<TranscribeReport, String> {
    // Предел по памяти, а не по часам: весь файл лежит в памяти f32 16 кГц,
    // поэтому отказ выдаём до декодирования, пока память ещё не занята.
    memory::check(audio_path)?;
    if !mr_audio::has_audio_track(audio_path) {
        return Err("Файл не содержит аудиодорожки".to_string());
    }
    // Видео: сначала извлекаем аудиодорожку (symphonia не берёт MKV/WebM/MOV/AVI).
    // Импортные видео не несут семантики «микрофон/система» — сводим их в моно.
    // Свои записи экрана хранят стерео-сплит и расшифровываются как аудио-записи.
    let video = is_video_path(audio_path);
    if video {
        on_event(1, "Извлекаю звук из видео…");
    }
    let decode_stage = Instant::now();
    stage_begin("decode");
    log::debug!(
        target: "audio",
        path = crate::logging::file_token(audio_path),
        bytes = std::fs::metadata(audio_path).map(|m| m.len()).unwrap_or(0);
        "ev=audio.read"
    );
    let decode_started = Instant::now();
    let channels = decode_to_channels_16k_auto(audio_path).map_err(|e| e.to_string())?;
    if channels.is_empty() || channels.iter().all(|c| c.is_empty()) {
        return Err("Файл не содержит аудиодорожки".to_string());
    }
    let duration_sec = channels.iter().map(|c| c.len()).max().unwrap_or(0) as f64 / 16000.0;
    let channel_stats: Vec<AudioStats> = if cfg!(debug_assertions) {
        channels.iter().map(|c| audio_stats(c)).collect()
    } else {
        Vec::new()
    };
    for (index, stats) in channel_stats.iter().enumerate() {
        log::debug!(
            target: "audio",
            ch = index as i64,
            samples = channels[index].len(),
            rms = stats.rms,
            peak = stats.peak,
            clip_pct = stats.clip_pct,
            silence_pct = stats.silence_pct,
            nan = stats.nan,
            inf = stats.inf;
            "ev=audio.channel"
        );
    }
    if cfg!(debug_assertions) {
        let aggregate = channel_stats
            .iter()
            .fold(AudioStats::default(), |mut acc, stats| {
                acc.rms = acc.rms.max(stats.rms);
                acc.peak = acc.peak.max(stats.peak);
                acc.clip_pct = acc.clip_pct.max(stats.clip_pct);
                acc.silence_pct = acc.silence_pct.max(stats.silence_pct);
                acc.nan += stats.nan;
                acc.inf += stats.inf;
                acc
            });
        log::info!(
            target: "run",
            bytes = std::fs::metadata(audio_path).map(|m| m.len()).unwrap_or(0),
            dur_sec = duration_sec,
            channels = channels.len(),
            rate = 16000,
            rms = aggregate.rms,
            peak = aggregate.peak,
            clip_pct = aggregate.clip_pct,
            silence_pct = aggregate.silence_pct,
            nan = aggregate.nan,
            inf = aggregate.inf,
            ch_samples = channels.iter().map(|c| c.len()).sum::<usize>();
            "ev=run.input"
        );
    }
    log::info!(
        target: "audio",
        file = crate::logging::file_token(audio_path),
        format = audio_path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default(),
        is_video = video,
        channels = channels.len(),
        dur_sec = duration_sec,
        elapsed_ms = decode_started.elapsed().as_millis() as u64;
        "ev=audio.decode"
    );
    stage_end("decode", decode_stage);
    let mut channels = if video && !crate::media_registry::load_registry().is_owned(audio_path) {
        let n = channels.iter().map(|c| c.len()).max().unwrap_or(0);
        let mut mono = vec![0.0f32; n];
        for ch in &channels {
            for (i, &x) in ch.iter().enumerate() {
                mono[i] += x / channels.len() as f32;
            }
        }
        vec![mono]
    } else {
        channels
    };
    let (mixed_channels, mix_stats) = channel_mix::mixed_stereo_with_stats(&channels);
    if channels.len() == 2 {
        log::info!(
            target: "audio",
            channels = 2,
            chosen = mix_stats.chosen.map(|c| c as i64).unwrap_or(-1),
            corr = mix_stats.corr,
            containment = mix_stats.containment,
            jaccard = mix_stats.jaccard,
            th_active0 = mix_stats.th0,
            th_active1 = mix_stats.th1;
            "ev=audio.mix"
        );
    }
    if let Some(pick) = mixed_channels {
        on_event(
            6,
            "Каналы дублируют одну и ту же речь — обрабатываю один канал",
        );
        let chosen = channels.remove(pick);
        channels = vec![chosen];
    }
    let is_stereo = channels.len() >= 2;
    let models_stage = Instant::now();
    stage_begin("models");
    events::emit_stage(2, "Загружаю модели…");
    let mut models = ModelsGuard::take()?;
    let use_gigaam = models.gigaam.is_some() && opts.engine != "whisper";
    let mut engine_name = if use_gigaam {
        "GigaAM-v3 (русский язык)".to_string()
    } else {
        "Whisper large-v3-turbo (30+ языков)".to_string()
    };
    events::emit_stage(6, &format!("Модели готовы: {engine_name}"));
    log::info!(
        target: "asr",
        engine = if use_gigaam { "gigaam" } else { "whisper" },
        model = if use_gigaam {
            "gigaam-v3-rnnt-int8"
        } else {
            "whisper-large-v3-turbo-q"
        },
        lang = opts.language.clone(),
        ctc = opts.ctc_enabled,
        hotwords_n = opts.hotwords.len();
        "ev=asr.engine"
    );
    if use_gigaam && !opts.hotwords.is_empty() {
        if !opts.ctc_enabled {
            log::warn!(
                target: "asr",
                reason = "ctc_disabled";
                "ev=asr.ctc.skip"
            );
            on_event(
                6,
                "Словарь терминов не применён: второй проход выключен в «Модели ИИ»",
            );
        } else if models.ctc.is_none() {
            log::warn!(
                target: "asr",
                reason = "model_missing";
                "ev=asr.ctc.skip"
            );
            on_event(
                6,
                "CTC-модель не скачана — словарь не применён. Скачайте её во вкладке «Компоненты»",
            );
        }
    }
    stage_end("models", models_stage);

    let (chan_list, speaker_method) = if is_stereo {
        (
            vec![
                (0usize, opts.speaker_left.clone()),
                (1, opts.speaker_right.clone()),
            ],
            "раздельные каналы записи (микрофон / система, наушники)",
        )
    } else if mixed_channels.is_some() {
        (
            vec![(0usize, opts.speaker_left.clone())],
            "общий микс (каналы дублируют речь, оставлен микрофон)",
        )
    } else {
        (vec![(0usize, opts.speaker_right.clone())], "общая запись")
    };

    // Каналы раскладываем по индексам без лишних копий.
    let mut channel_samples: HashMap<usize, Vec<f32>> = channels
        .into_iter()
        .enumerate()
        .filter(|(i, _)| chan_list.iter().any(|(ch, _)| ch == i))
        .collect();

    // Выравниваем громкость скользящим RMS перед VAD и ASR, чтобы тихие реплики не отсекались порогом.
    for samples in channel_samples.values_mut() {
        mr_audio::dsp::agc_rms_16k(samples);
    }

    let (channel_vad, channel_grouped) = vad::run(
        &channel_samples,
        &chan_list,
        is_stereo,
        &mut models.vad,
        on_event,
    )?;
    let (channel_utts, ctc_used_any) = asr::run(
        &channel_grouped,
        &channel_samples,
        &chan_list,
        opts,
        use_gigaam,
        &mut models,
        on_event,
    )?;
    let (speakers_info, merged, speakers) = speakers::run(speakers::Input {
        channel_samples: &channel_samples,
        chan_list: &chan_list,
        channel_vad: &channel_vad,
        channel_utts,
        models: &mut models,
        opts,
        is_stereo,
        on_event,
    });
    let words = merged
        .iter()
        .map(|u| u.text.split_whitespace().count())
        .sum::<usize>();
    let speech_sec = unique_speech_sec(&merged);

    let stem = audio_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "meeting".to_string());
    let out_dir = audio_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let method = if is_stereo {
        "Микрофон и система, наушники (модели: VAD, GigaAM, векторы голосов)".to_string()
    } else {
        speaker_method.to_string() + " + разделение голосов"
    };
    if ctc_used_any {
        engine_name.push_str(" + CTC-уточнение");
    }
    let hotwords_applied = sanitize_hotwords(&opts.hotwords);
    let speakers_info_json: Vec<serde_json::Value> =
        speakers_info.iter().map(report::speaker_to_json).collect();
    let meta = Meta {
        audio: audio_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        is_video: video,
        duration: report::human_duration(duration_sec),
        engine: engine_name.clone(),
        speaker_method: method.clone(),
        speakers: speakers.clone(),
        speakers_info: speakers_info_json,
        hotwords_applied: hotwords_applied.clone(),
        confidence: Some(
            serde_json::json!({"engine": "ctc", "threshold": DEFAULT_CONFIDENCE_THRESHOLD}),
        ),
    };
    let doc = TranscriptDoc {
        schema: super::document::SCHEMA_VERSION,
        meta,
        stats: Stats {
            segments: merged.len(),
            words,
            speech_sec: (speech_sec * 10.0).round() / 10.0,
            diarized: true,
        },
        utterances: merged,
    };
    let meta_value =
        serde_json::to_value(&doc.meta).map_err(|e| format!("Метаданные расшифровки: {e}"))?;
    let md_path = out_dir.join(format!("{stem}.md"));
    let txt_path = out_dir.join(format!("{stem}.txt"));
    let json_path = out_dir.join(format!("{stem}.transcript.json"));
    crate::fsutil::write_atomic(
        &md_path,
        report::build_markdown(&meta_value, &doc.utterances).as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    crate::fsutil::write_atomic(&txt_path, report::build_text(&doc.utterances).as_bytes())
        .map_err(|e| e.to_string())?;
    if cfg!(debug_assertions) {
        let input_stats = channel_stats
            .iter()
            .fold(AudioStats::default(), |mut acc, stats| {
                acc.rms = acc.rms.max(stats.rms);
                acc.peak = acc.peak.max(stats.peak);
                acc.clip_pct = acc.clip_pct.max(stats.clip_pct);
                acc.silence_pct = acc.silence_pct.max(stats.silence_pct);
                acc.nan += stats.nan;
                acc.inf += stats.inf;
                acc
            });
        write_run_manifest(
            audio_path,
            opts,
            duration_sec,
            channel_stats.len(),
            input_stats,
            &engine_name,
        );
    }
    write_transcript_doc(&json_path, &doc)?;
    report::record_stats(duration_sec);
    Ok(TranscribeReport {
        md_path,
        txt_path,
        json_path,
        utterances: doc.utterances.len(),
        speakers,
        words,
        speech_sec,
        engine: engine_name.to_string(),
        speaker_method: method,
        duration_sec,
    })
}

#[cfg(test)]
mod tests {
    use super::quiet_blank_penalty;

    #[test]
    fn blank_penalty_only_for_short_quiet_speech() {
        assert_eq!(quiet_blank_penalty(2.6, 0.011), 2.0);
        assert_eq!(quiet_blank_penalty(10.0, 0.011), 0.0);
        assert_eq!(quiet_blank_penalty(2.6, 0.0003), 0.0);
        assert_eq!(quiet_blank_penalty(2.6, 0.2), 0.0);
    }
}
