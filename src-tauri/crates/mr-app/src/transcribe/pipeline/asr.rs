use std::collections::HashMap;
use std::time::Instant;

use mr_ml::dictionary::Dictionary;
use mr_ml::hotwords::confidence_threshold;
use mr_ml::onnx::ctc::align_to_reference_timed;

use super::super::diag::{
    asr_loop_metrics, mel_stats, metrics_look_like_loop, percentile, stage_begin, stage_end,
};
use super::super::diarize;
use super::super::models::ModelSet;
use super::super::perf;
use super::super::{TranscribeOptions, Utterance, WordConf};
use super::quiet_blank_penalty;

pub(super) fn run(
    channel_grouped: &HashMap<usize, Vec<(f64, f64)>>,
    channel_samples: &HashMap<usize, Vec<f32>>,
    chan_list: &[(usize, String)],
    opts: &TranscribeOptions,
    use_gigaam: bool,
    models: &mut ModelSet,
    on_event: &dyn Fn(u8, &str),
) -> Result<(HashMap<usize, Vec<Utterance>>, bool), String> {
    let total_segs: usize = channel_grouped.values().map(|v| v.len()).sum();
    let mut done_segs = 0usize;
    let mut seg_index = 0usize;
    let mut ctc_used_any = false;
    let mut channel_utts: HashMap<usize, Vec<Utterance>> = HashMap::new();
    let ctc_available = use_gigaam && opts.ctc_enabled && models.ctc.is_some();
    let dictionary = Dictionary::new(&opts.hotwords);
    let mut dictionary_utts = 0usize;
    let mut dictionary_words = 0usize;
    if !ctc_available && total_segs > 0 {
        log::info!(
            target: "asr",
            gigaam = use_gigaam,
            ctc_enabled = opts.ctc_enabled,
            ctc_loaded = models.ctc.is_some();
            "ev=asr.word_timestamps_unavailable"
        );
    }
    let asr_stage = Instant::now();
    let _perf = perf::stage("asr");
    stage_begin("asr");
    for (ch, label) in chan_list.iter() {
        let samples = &channel_samples[ch];
        let mut utts = vec![];
        for (s, e) in channel_grouped[ch].iter() {
            let a = *s;
            let b = e.min(samples.len() as f64 / 16000.0);
            if b <= a {
                log::debug!(
                    target: "asr",
                    i = seg_index,
                    reason = "degenerate_after_padding";
                    "ev=asr.empty"
                );
                seg_index += 1;
                continue; // вырожденный сегмент после отсечения границей файла
            }
            let lo = (a * 16000.0) as usize;
            let hi = ((b * 16000.0) as usize).min(samples.len());
            let raw_chunk = &samples[lo..hi];

            // По-чанковая нормализация до пика 0.9 перед ASR:
            // полный динамический диапазон для INT8 квантованных моделей (GigaAM / Whisper).
            const TARGET_PEAK: f32 = 0.9;
            let mut peak = 0.0f32;
            let mut sum_sq = 0.0f32;
            for &x in raw_chunk {
                peak = peak.max(x.abs());
                sum_sq += x * x;
            }
            let rms_raw = (sum_sq / raw_chunk.len().max(1) as f32).sqrt();
            // Пол по RMS, а не по пику: один щелчок задаёт пик, но не энергию.
            // Ниже 1e-5 — только шум квантования 16-бит (LSB/√12 ≈ 9e-6) и
            // дизеринг; их растяжение до пика 0.9 даёт галлюцинации ASR на
            // ложных VAD-сегментах. Реальная тихая речь — от 1e-4, запас 10×.
            const QUIET_RMS_FLOOR: f32 = 1e-5;
            let normalized_buf: Vec<f32>;
            let chunk: &[f32] = if rms_raw > QUIET_RMS_FLOOR && (peak - TARGET_PEAK).abs() > 0.05 {
                let scale = TARGET_PEAK / peak;
                normalized_buf = raw_chunk
                    .iter()
                    .map(|&x| (x * scale).clamp(-1.0, 1.0))
                    .collect();
                &normalized_buf
            } else {
                raw_chunk
            };
            let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
            // Штраф — по RMS сырого сегмента: после нормализации до пика 0.9
            // любая речь даёт 0.09–0.6, и ветка «тихая короткая речь» недостижима.
            let blank_penalty = if use_gigaam {
                quiet_blank_penalty(*e - *s, rms_raw)
            } else {
                0.0
            };
            log::trace!(
                target: "asr",
                i = seg_index,
                dur_sec = *e - *s,
                rms = rms,
                blank_penalty = blank_penalty;
                "ev=asr.blank_penalty"
            );
            let mel = use_gigaam.then(|| mr_ml::onnx::features::nemo_mel(chunk));
            let mel_frames = mel.as_ref().map(|m| m.len()).unwrap_or(0);
            if log::log_enabled!(target: "asr", log::Level::Debug) {
                let (mel_min, mel_max, mel_nan) =
                    mel.as_deref().map(mel_stats).unwrap_or((0.0, 0.0, 0));
                log::debug!(
                    target: "asr",
                    i = seg_index,
                    ch = *ch as i64,
                    start_sec = a,
                    end_sec = b,
                    dur_sec = b - a,
                    rms = rms,
                    blank_penalty = blank_penalty,
                    mel_frames = mel_frames,
                    mel_min = mel_min,
                    mel_max = mel_max,
                    mel_nan = mel_nan;
                    "ev=asr.seg.start"
                );
            }
            let infer_started = Instant::now();
            let text = if use_gigaam {
                // Тихая речь: жадный RNNT предпочитает blank и теряет слова.
                // Штраф включаем только для коротких фрагментов с речевой
                // громкостью: на длинном шуме и почти-тишине он заставляет
                // декодер выдумывать повторяющийся текст.
                models
                    .gigaam
                    .as_mut()
                    .ok_or("нет GigaAM-модели")?
                    .transcribe_mel_penalized(mel.as_deref().unwrap_or(&[]), blank_penalty)
                    .map_err(|error| {
                        log::error!(
                            target: "asr",
                            i = seg_index,
                            model = "gigaam-v3-rnnt-int8",
                            mel_frames = mel_frames,
                            error = error;
                            "ev=asr.infer_error"
                        );
                        format!("GigaAM: {error}")
                    })?
            } else {
                models
                    .whisper
                    .as_mut()
                    .ok_or("нет Whisper-модели")?
                    .transcribe_segment(chunk, &opts.language)
                    .map_err(|error| {
                        log::error!(
                            target: "asr",
                            i = seg_index,
                            model = "whisper-large-v3-turbo-q",
                            mel_frames = mel_frames,
                            error = error;
                            "ev=asr.infer_error"
                        );
                        format!("Whisper: {error}")
                    })?
            };
            let infer_ms = infer_started.elapsed().as_millis() as u64;
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let metrics = asr_loop_metrics(&text);
            let looped = metrics_look_like_loop(metrics);
            if looped {
                log::warn!(
                    target: "asr",
                    i = seg_index,
                    max_run = metrics.max_run,
                    unique_ratio = metrics.unique_ratio,
                    max_count = metrics.max_count,
                    words = metrics.words,
                    text_preview = crate::logging::text_preview(&text);
                    "ev=asr.loop"
                );
            }
            if text.is_empty() {
                log::debug!(
                    target: "asr",
                    i = seg_index,
                    reason = "empty_text";
                    "ev=asr.empty"
                );
            } else if looped {
                log::debug!(
                    target: "asr",
                    i = seg_index,
                    reason = "loop";
                    "ev=asr.empty"
                );
            }
            let decode_stats = if use_gigaam {
                models.gigaam.as_ref().map(|model| model.last_stats())
            } else {
                None
            };
            let (tokens, blank_ratio) = decode_stats
                .map(|stats| {
                    (
                        stats.tokens,
                        if stats.frames == 0 {
                            0.0
                        } else {
                            stats.blank as f64 / stats.frames as f64
                        },
                    )
                })
                .unwrap_or((0, 0.0));
            done_segs += 1;
            let pct = (15.0 + 65.0 * done_segs as f64 / total_segs.max(1) as f64) as u8;
            // Второй проход CTC идёт внутри того же счётчика сегментов,
            // поэтому прогресс его уже учитывает без отдельных стадий.
            on_event(
                pct.min(80),
                &format!("Реплик распознано: {done_segs} из {total_segs}"),
            );
            let mut ctc_applied = 0usize;
            let mut ctc_skipped = 0usize;
            let mut ctc_changed = 0usize;
            let mut ctc_needs_review = 0usize;
            let mut ctc_hyp_chars = 0usize;
            let mut ctc_conf_p50 = 0.0f32;
            let mut ctc_conf_min = 0.0f32;
            let mut words: Vec<WordConf> = Vec::new();
            let mut ctc_used = false;
            let mut final_text = text;
            // Зацикленный вывод жадного декодера на шуме — не речь.
            if !final_text.is_empty() && !looped {
                if use_gigaam && opts.ctc_enabled {
                    if let Some(ctc) = models.ctc.as_mut() {
                        match ctc
                            .transcribe_mel_timed(mel.as_deref().unwrap_or(&[]), &opts.hotwords)
                        {
                            Ok((hyp, times)) => {
                                ctc_hyp_chars = hyp.text.chars().count();
                                if hyp.words.is_empty() {
                                    ctc_skipped += 1;
                                    log::debug!(
                                        target: "asr",
                                        i = seg_index,
                                        reason = "empty_hypothesis";
                                        "ev=asr.ctc.skip"
                                    );
                                } else {
                                    let original: Vec<&str> =
                                        final_text.split_whitespace().collect();
                                    let aligned = align_to_reference_timed(
                                        &final_text,
                                        &hyp,
                                        &times,
                                        &opts.hotwords,
                                        confidence_threshold(),
                                    );
                                    ctc_changed = aligned
                                        .iter()
                                        .zip(original.iter())
                                        .filter(|(word, original)| word.text != **original)
                                        .count()
                                        + aligned.len().saturating_sub(original.len());
                                    ctc_needs_review =
                                        aligned.iter().filter(|word| word.needs_review).count();
                                    let mut confs: Vec<f32> =
                                        aligned.iter().map(|w| w.confidence as f32).collect();
                                    confs.sort_by(f32::total_cmp);
                                    ctc_conf_p50 = percentile(&confs, 0.5);
                                    ctc_conf_min = percentile(&confs, 0.0);
                                    final_text = aligned
                                        .iter()
                                        .map(|w| w.text.clone())
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    words = aligned
                                        .into_iter()
                                        .map(|w| WordConf {
                                            w: w.text,
                                            conf: (w.confidence * 1000.0).round() / 1000.0,
                                            needs_review: w.needs_review,
                                            start_sec: if w.end_sec > w.start_sec {
                                                a + w.start_sec
                                            } else {
                                                0.0
                                            },
                                            end_sec: if w.end_sec > w.start_sec {
                                                a + w.end_sec
                                            } else {
                                                0.0
                                            },
                                        })
                                        .collect();
                                    ctc_applied = 1;
                                    ctc_used = true;
                                    ctc_used_any = true;
                                }
                            }
                            Err(error) => {
                                ctc_skipped += 1;
                                log::warn!(
                                    target: "asr",
                                    i = seg_index,
                                    reason = "infer_error",
                                    error = error;
                                    "ev=asr.ctc.skip"
                                );
                            }
                        }
                    }
                }
                if use_gigaam && opts.ctc_enabled {
                    log::debug!(
                        target: "asr",
                        i = seg_index,
                        hotwords_n = opts.hotwords.len(),
                        applied = ctc_applied,
                        skipped = ctc_skipped,
                        hyp_chars = ctc_hyp_chars,
                        changed_words = ctc_changed,
                        needs_review = ctc_needs_review,
                        conf_p50 = ctc_conf_p50,
                        conf_min = ctc_conf_min;
                        "ev=asr.ctc"
                    );
                }
                let replaced = apply_dictionary(&dictionary, ctc_used, &mut final_text);
                if replaced > 0 {
                    dictionary_utts += 1;
                    dictionary_words += replaced;
                }
            }
            // Калибровка диаризации: эмбеддинги шума без текста создают ложные кластеры дикторов.
            let words_count = final_text.split_whitespace().count();
            if !final_text.trim().is_empty() && !looped {
                utts.push(Utterance {
                    start: (a * 100.0).round() / 100.0,
                    end: (b * 100.0).round() / 100.0,
                    speaker: label.clone(),
                    channel: Some(*ch as i32),
                    text: final_text,
                    diarized: false,
                    silhouette: 0.0,
                    words,
                    ctc_used,
                });
                log::debug!(
                    target: "asr",
                    i = seg_index,
                    ch = *ch as i64,
                    dur_ms = infer_ms,
                    tokens = tokens,
                    blank_ratio = blank_ratio,
                    chars = utts.last().map(|u| u.text.chars().count()).unwrap_or(0),
                    words = words_count,
                    text_preview = utts
                        .last()
                        .map(|u| crate::logging::text_preview(&u.text))
                        .unwrap_or_default(),
                    loop = looped;
                    "ev=asr.seg.done"
                );
                if let Some(utterance) = utts.last() {
                    if cfg!(debug_assertions) && crate::logging::dump_enabled() {
                        crate::logging::append_artifact_jsonl(
                            "asr/segments.jsonl",
                            &serde_json::json!({
                                "i": seg_index,
                                "ch": *ch as i32,
                                "start_sec": utterance.start,
                                "end_sec": utterance.end,
                                "rms": rms,
                                "blank_penalty": blank_penalty,
                                "mel_frames": mel_frames,
                                "tokens": tokens,
                                "blank_ratio": blank_ratio,
                                "chars": utterance.text.chars().count(),
                                "words": utterance.text.split_whitespace().count(),
                                "text": crate::logging::text_preview(&utterance.text),
                                "ctc_used": utterance.ctc_used,
                                "ctc_applied": ctc_applied,
                                "ctc_skipped": ctc_skipped,
                                "ctc_changed_words": ctc_changed,
                                "infer_ms": infer_ms,
                            }),
                        );
                    }
                    log::trace!(
                        target: "asr",
                        i = seg_index,
                        text = crate::logging::text_preview(&utterance.text);
                        "ev=asr.text"
                    );
                }
            } else {
                log::debug!(
                    target: "asr",
                    i = seg_index,
                    ch = *ch as i64,
                    dur_ms = infer_ms,
                    tokens = tokens,
                    blank_ratio = blank_ratio,
                    chars = final_text.chars().count(),
                    words = words_count,
                    text_preview = crate::logging::text_preview(&final_text),
                    loop = looped;
                    "ev=asr.seg.skip"
                );
            }
            seg_index += 1;
        }
        diarize::move_boundary_words(&mut utts);
        channel_utts.insert(*ch, utts);
    }
    stage_end("asr", asr_stage);
    if dictionary.alias_count() > 0 {
        log::debug!(
            target: "asr",
            aliases = dictionary.alias_count(),
            terms = dictionary.term_count(),
            utts = dictionary_utts,
            words = dictionary_words;
            "ev=asr.dict"
        );
    }

    Ok((channel_utts, ctc_used_any))
}

fn apply_dictionary(dictionary: &Dictionary, ctc_used: bool, text: &mut String) -> usize {
    if ctc_used {
        0
    } else {
        dictionary.apply(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_is_skipped_after_ctc_and_applied_otherwise() {
        let dictionary = Dictionary::new(&["мерши => merge".to_string()]);
        let mut text = "мерши".to_string();
        assert_eq!(apply_dictionary(&dictionary, true, &mut text), 0);
        assert_eq!(text, "мерши");
        assert_eq!(apply_dictionary(&dictionary, false, &mut text), 1);
        assert_eq!(text, "merge");
    }
}
