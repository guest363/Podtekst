//! Сквозной нативный ONNX-пайплайн транскрибации:
//! VAD (Silero) → ASR (GigaAM-v3 RNNT / Whisper-turbo) → диаризация
//! (PyAnnote-маски + ERes2NetV2 + кластеризация + Венгерский алгоритм)
//! → сопоставление с Voice Packs → `.md` / `.txt` / `.transcript.json`.
//!
//! * [`events`] — SSE-шина и фоновый воркер
//! * [`diarize`] — кластеризация, профили спикеров
//! * [`report`] — выходные форматы и статистика

pub mod diarize;
pub mod events;
pub mod memory;
pub mod report;
mod speaker_assignment;
mod word_speakers;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use mr_audio::decode::{decode_to_channels_16k_auto, is_video_path};
use mr_packs as packs;

use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::rnnt::GigaAM;
use mr_ml::onnx::segmentation::SegmentationModel;
use mr_ml::onnx::vad::{vad_segments_from_probs, SileroVad};
use mr_ml::onnx::whisper::Whisper;

pub use events::{
    emit_model_event, finish_model_events, is_transcribe_running, spawn_transcription_worker,
    start_model_download, take_new_events, transcribing_file,
};

// ---------- Модели ----------

fn models_root() -> PathBuf {
    packs::default_app_data_dir().join("models")
}

fn find_model_file(model_id: &str, needle: &str) -> Option<PathBuf> {
    let dir = models_root().join(model_id);
    std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        let n = p.file_name()?.to_string_lossy().to_lowercase();
        (p.is_file() && n.contains(needle)).then_some(p)
    })
}

/// Загруженный набор ONNX-моделей пайплайна.
pub struct ModelSet {
    vad: SileroVad,
    gigaam: Option<GigaAM>,
    whisper: Option<Whisper>,
    embed: Option<EmbeddingExtractor>,
    segmentation: Option<SegmentationModel>,
}

/// Кэш моделей процесса: веса в сотни МБ читаются с диска один раз,
/// а не на каждую расшифровку. Конкуренция исключена флагом RUNNING.
static MODELS_CACHE: OnceLock<Mutex<Option<ModelSet>>> = OnceLock::new();

fn take_cached_models() -> Result<ModelSet, String> {
    let cache = MODELS_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(ModelSet::load()?);
    }
    Ok(guard.take().expect("только что положили"))
}

fn return_cached_models(models: ModelSet) {
    let cache = MODELS_CACHE.get_or_init(|| Mutex::new(None));
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(models);
}

impl ModelSet {
    /// Загрузка VAD + выбранного ASR + моделей диаризации из `<app_data>/models`.
    pub fn load() -> Result<Self, String> {
        let vad_path = find_model_file("vad-silero-v6", ".onnx")
            .ok_or_else(|| "Модель Silero VAD не скачана. Откройте «Настройки» → «Модели ИИ» и нажмите «Скачать».".to_string())?;
        let vad = SileroVad::load(&vad_path)?;
        let giga_dir = models_root().join("asr-gigaam-v3-rnnt-int8");
        let gigaam = GigaAM::load(&giga_dir).ok();
        let whisper_dir = models_root().join("asr-whisper-large-v3-turbo-q");
        let whisper = Whisper::load(&whisper_dir).ok();
        let emb_path = find_model_file("diar-eres2netv2-emb", ".onnx")
            .or_else(|| find_model_file("diar-pyannote-emb", ".onnx"))
            .or_else(|| find_model_file("diar-wespeaker-resnet34", ".onnx"));
        let embed = emb_path.and_then(|p| EmbeddingExtractor::load(&p).ok());
        let segmentation = find_model_file("diar-pyannote-segmentation-3", ".onnx")
            .and_then(|p| SegmentationModel::load(&p).ok());
        if gigaam.is_none() && whisper.is_none() {
            return Err("Нет модели распознавания речи. Скачайте GigaAM или Whisper: «Настройки» → «Модели ИИ».".to_string());
        }
        Ok(Self {
            vad,
            gigaam,
            whisper,
            embed,
            segmentation,
        })
    }
}

// ---------- Структуры результата ----------

/// Одна распознанная реплика.
#[derive(Debug, Clone)]
pub struct Utterance {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub channel: Option<i32>,
    pub text: String,
    pub diarized: bool,
    /// Силуэт назначения голоса: -1..1, больше — увереннее.
    pub silhouette: f64,
}

/// Итог диаризации по одному спикеру.
#[derive(Debug, Clone)]
pub struct SpeakerInfo {
    pub id: String,
    pub name: String,
    pub channel: Option<i32>,
    pub matched: bool,
    pub matched_name: Option<String>,
    pub similarity: f64,
    /// Взвешенный по длительности силуэт фрагментов спикера.
    pub silhouette: f64,
    pub candidates: Vec<(String, f64)>,
    pub embedding: Option<Vec<f64>>,
    pub samples: Vec<Sample>,
    pub utterances_count: usize,
    pub total_speech_sec: f64,
}

/// Образец речи спикера для прослушивания в UI.
#[derive(Debug, Clone)]
pub struct Sample {
    pub start: f64,
    pub end: f64,
    pub dur: f64,
    pub text: String,
    pub full_text: String,
    /// Канал исходной записи: фрагмент проигрывается изолированно.
    pub channel: Option<i32>,
}

/// Параметры запуска транскрибации.
pub struct TranscribeOptions {
    pub speaker_left: String,
    pub speaker_right: String,
    pub engine: String,
    /// Язык Whisper в виде кода («ru», «en»…); «auto» — определение моделью.
    /// GigaAM игнорирует поле (он одноязычный, русский).
    pub language: String,
    pub min_speakers: usize,
    pub max_speakers: usize,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            speaker_left: "Я".to_string(),
            speaker_right: "Собеседник".to_string(),
            engine: "auto".to_string(),
            language: "auto".to_string(),
            min_speakers: 0,
            max_speakers: 0,
        }
    }
}

/// Сводка выполненной расшифровки (для CLI-вывода).
pub struct TranscribeReport {
    pub md_path: PathBuf,
    pub txt_path: PathBuf,
    pub json_path: PathBuf,
    pub utterances: usize,
    pub speakers: Vec<String>,
    pub words: usize,
    pub speech_sec: f64,
    pub engine: String,
    pub speaker_method: String,
    pub duration_sec: f64,
}

/// Параметры окон ASR над VAD-сегментами.
const MAX_SEGMENT_SEC: f32 = 20.0;
const SEGMENT_GAP_SEC: f32 = 0.5;
const SEGMENT_PAD_SEC: f64 = 0.25;

/// Штраф на blank только для короткой тихой речи. На длинных фрагментах,
/// почти-тишине и громкой речи штраф выключен: иначе декодер выдумывает
/// повторяющийся текст на шуме.
fn quiet_blank_penalty(duration_sec: f64, rms: f32) -> f32 {
    if duration_sec <= 4.0 && (0.0015..0.015).contains(&rms) {
        2.0
    } else {
        0.0
    }
}

/// Признак зацикливания ASR: один и тот же токен/слог повторяется десятки раз
/// или текст почти полностью состоит из нескольких слов. Такой фрагмент —
/// галлюцинация декодера на шуме, а не речь; в отчёт он не попадает.
fn looks_like_asr_loop(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.len() < 8 {
        return false;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut max_run = 1usize;
    let mut run = 1usize;
    for (i, w) in words.iter().enumerate() {
        *counts.entry(w.as_str()).or_default() += 1;
        if i > 0 {
            run = if words[i] == words[i - 1] { run + 1 } else { 1 };
            max_run = max_run.max(run);
        }
    }
    let unique_ratio = counts.len() as f32 / words.len() as f32;
    let max_count = counts.values().copied().max().unwrap_or(0);
    max_run >= 8
        || (max_run >= 6 && unique_ratio < 0.4)
        || (max_count >= 10 && max_count as f32 / words.len() as f32 >= 0.35)
        || (words.len() >= 20 && unique_ratio < 0.22)
}

/// Полный проход: файл → реплики с именами спикеров → md/txt/transcript.json.
pub fn transcribe_file(
    audio_path: &Path,
    opts: &TranscribeOptions,
    on_event: &dyn Fn(u8, &str),
) -> Result<TranscribeReport, String> {
    // Предел по памяти, а не по часам: весь файл лежит в памяти f32 16 кГц,
    // поэтому отказ выдаём до декодирования, пока память ещё не занята.
    memory::check(audio_path)?;
    // Видео: сначала извлекаем аудиодорожку (symphonia не берёт MKV/WebM/MOV/AVI),
    // затем сводим в моно — каналов «микрофон/система» у видеофайла нет.
    let video = is_video_path(audio_path);
    if video {
        on_event(1, "Извлекаю звук из видео…");
    }
    let channels = decode_to_channels_16k_auto(audio_path).map_err(|e| e.to_string())?;
    if channels.is_empty() || channels.iter().all(|c| c.is_empty()) {
        return Err("в файле нет звуковой дорожки".to_string());
    }
    let channels = if video {
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
    let is_stereo = channels.len() >= 2;
    let duration_sec = channels
        .first()
        .map(|c| c.len() as f64 / 16000.0)
        .unwrap_or(0.0);
    events::emit_stage(2, "Загружаю модели…");
    let mut models = take_cached_models()?;
    let engine_name = if models.gigaam.is_some() && opts.engine != "whisper" {
        "GigaAM-v3 (e2e_rnnt, onnx-int8)"
    } else {
        "Whisper large-v3-turbo (onnx, merged-decoder)"
    };
    events::emit_stage(6, &format!("Модели готовы: {engine_name}"));
    let use_gigaam = engine_name.starts_with("GigaAM");

    // Каналы: стерео — раздельные каналы записи (микрофон / система), моно — общий поток.
    let (chan_list, speaker_method) = if is_stereo {
        (
            vec![
                (0usize, opts.speaker_left.clone()),
                (1, opts.speaker_right.clone()),
            ],
            "раздельные каналы записи (микрофон / система)",
        )
    } else {
        (vec![(0usize, opts.speaker_right.clone())], "общая запись")
    };

    // Каналы отдаём как есть: `channels` больше нигде не нужен, поэтому
    // раскладываем его по индексам без копий (иначе длинная запись занимала
    // вдвое больше памяти).
    let channel_samples: HashMap<usize, Vec<f32>> = channels
        .into_iter()
        .enumerate()
        .filter(|(i, _)| chan_list.iter().any(|(ch, _)| ch == i))
        .collect();

    // VAD + группировка
    let mut channel_grouped: HashMap<usize, Vec<(f64, f64)>> = HashMap::new();
    let channel_count = chan_list.len().max(1) as f64;
    for (ch_i, (ch, label)) in chan_list.iter().enumerate() {
        let samples = &channel_samples[ch];
        let base = 6.0 + 8.0 * ch_i as f64 / channel_count;
        let span = 8.0 / channel_count;
        let probs = models
            .vad
            .process_all_with(samples, |done, total| {
                let pct = (base + span * done as f64 / total.max(1) as f64) as u8;
                on_event(
                    pct.clamp(6, 14),
                    &format!("Поиск речи ({label}): {done} из {total} окон"),
                );
            })
            .map_err(|e| format!("VAD: {e}"))?;
        let segs = vad_segments_from_probs(&probs, 0.1, 0.25, 0.5);
        let grouped = diarize::merge_quiet_short(
            samples,
            diarize::group_segments(&segs, &probs, MAX_SEGMENT_SEC, SEGMENT_GAP_SEC),
        );
        on_event(
            (base + span) as u8,
            &format!("{label}: найдено {} фрагментов", grouped.len()),
        );
        channel_grouped.insert(
            *ch,
            grouped
                .into_iter()
                .map(|(a, b)| (a as f64, b as f64))
                .collect(),
        );
    }
    let total_segs: usize = channel_grouped.values().map(|v| v.len()).sum();
    let mut done_segs = 0usize;
    let mut channel_utts: HashMap<usize, Vec<Utterance>> = HashMap::new();
    for (ch, label) in chan_list.iter() {
        let samples = &channel_samples[ch];
        let mut utts = vec![];
        for (s, e) in channel_grouped[ch].iter() {
            let a = (s - SEGMENT_PAD_SEC).max(0.0);
            let b = (e + SEGMENT_PAD_SEC).min(samples.len() as f64 / 16000.0);
            if b <= a {
                continue; // вырожденный сегмент после отсечения границей файла
            }
            let lo = (a * 16000.0) as usize;
            let hi = ((b * 16000.0) as usize).min(samples.len());
            let chunk = &samples[lo..hi];
            let text = if use_gigaam {
                // Тихая речь: жадный RNNT предпочитает blank и теряет слова.
                // Штраф включаем только для коротких фрагментов с речевой
                // громкостью: на длинном шуме и почти-тишине он заставляет
                // декодер выдумывать повторяющийся текст.
                let rms =
                    (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
                // Длительность — до padding-а, иначе короткая реплика у границы
                // 4 секунд теряет штраф из-за добавленных 0.5 с.
                let blank_penalty = quiet_blank_penalty(*e - *s, rms);
                models
                    .gigaam
                    .as_mut()
                    .ok_or("нет GigaAM-модели")?
                    .transcribe_segment_penalized(chunk, blank_penalty)
                    .map_err(|e| format!("GigaAM: {e}"))?
            } else {
                models
                    .whisper
                    .as_mut()
                    .ok_or("нет Whisper-модели")?
                    .transcribe_segment(chunk, &opts.language)
                    .map_err(|e| format!("Whisper: {e}"))?
            };
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            done_segs += 1;
            let pct = (15.0 + 65.0 * done_segs as f64 / total_segs.max(1) as f64) as u8;
            on_event(
                pct.min(80),
                &format!("Реплик распознано: {done_segs} из {total_segs}"),
            );
            // Зацикленный вывод жадного декодера на шуме — не речь.
            if !text.is_empty() && !looks_like_asr_loop(&text) {
                utts.push(Utterance {
                    start: (a * 100.0).round() / 100.0,
                    end: (b * 100.0).round() / 100.0,
                    speaker: label.clone(),
                    channel: Some(*ch as i32),
                    text,
                    diarized: false,
                    silhouette: 0.0,
                });
            }
        }
        channel_utts.insert(*ch, utts);
    }

    // Диаризация
    let profiles = diarize::load_profiles();
    // Заимствуем модели на время диаризации; вернём в кэш перед выходом.
    let mut embed_ext = models.embed.take();
    let mut segmenter = models.segmentation.take();
    let mut speakers_info: Vec<SpeakerInfo> = vec![];
    let mut all_utts: Vec<Utterance> = vec![];
    for (ch, _label) in chan_list.iter() {
        let samples = &channel_samples[ch];
        let source_utts = channel_utts.remove(ch).unwrap_or_default();
        let segmentation = if embed_ext.is_some() && !source_utts.is_empty() {
            on_event(80, "Ищу одновременную речь…");
            segmenter
                .as_mut()
                .and_then(|model| match model.segment(samples) {
                    Ok(result) => Some(result),
                    Err(error) => {
                        eprintln!("PyAnnote segmentation недоступна: {error}");
                        None
                    }
                })
        } else {
            None
        };
        // В стерео каналы уже разделены по голосам, и прежний разрез по
        // эмбеддингам там не трогаем. В моно реплики режем по паузам:
        // иначе 20 секунд сплошного диалога дают «средний голос» двух людей.
        let mut diar_utts = if is_stereo {
            on_event(80, "Ищу смену голоса внутри длинных реплик…");
            diarize::split_long_by_speaker_change(
                samples,
                source_utts.clone(),
                &mut embed_ext,
                &|pct, msg| on_event(pct, msg),
            )
        } else {
            on_event(80, "Делю речь на паузы для спикеров…");
            diarize::split_at_pauses(samples, &source_utts)
        };
        let (prefix, ch_idx) = if is_stereo {
            (
                if *ch == 0 {
                    "Участник"
                } else {
                    "Собеседник"
                },
                Some(*ch as i32),
            )
        } else {
            ("Собеседник", None)
        };
        let default_name = if *ch == 0 {
            opts.speaker_left.clone()
        } else {
            opts.speaker_right.clone()
        };
        let infos = diarize::diarize_channel(
            samples,
            &mut diar_utts,
            ch_idx,
            &default_name,
            prefix,
            &mut embed_ext,
            segmentation.as_ref(),
            &profiles,
            opts.min_speakers,
            opts.max_speakers,
            &|pct, msg| on_event(pct, msg),
        );
        speakers_info.extend(infos);
        let tree = word_speakers::DiarIntervalTree::new(&diar_utts);
        for utterance in &source_utts {
            all_utts.extend(word_speakers::split_utterance_by_word_speakers(
                utterance, &tree,
            ));
        }
    }
    // Один и тот же голос в микрофоне и системе — одна карточка, а не два
    // разных «Участник N» / «Собеседник N». Дубли эха схлопываем до одной фразы.
    diarize::reconcile_channels(&mut speakers_info, &mut all_utts);
    diarize::dedupe_cross_channel(&mut all_utts);
    all_utts.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.speaker.cmp(&b.speaker)));
    let merged = diarize::merge_utterances(all_utts);
    diarize::refresh_speaker_stats(&mut speakers_info, &merged);
    let words = merged
        .iter()
        .map(|u| u.text.split_whitespace().count())
        .sum::<usize>();
    // Уникальное покрытие таймлайна: сумма по каналам давала «232 с» в
    // трёхминутной встрече из-за одновременной речи микрофона и системы.
    let mut spans: Vec<(f64, f64)> = merged
        .iter()
        .map(|u| (u.start.max(0.0), u.end.max(u.start)))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut covered = 0.0;
    let mut cur: Option<(f64, f64)> = None;
    for (s, e) in spans {
        match cur {
            None => cur = Some((s, e)),
            Some((a, b)) if s <= b => cur = Some((a, b.max(e))),
            Some((a, b)) => {
                covered += b - a;
                cur = Some((s, e));
            }
        }
    }
    if let Some((a, b)) = cur {
        covered += b - a;
    }
    let speech_sec = covered;

    let speakers: Vec<String> = {
        let mut s = vec![];
        for u in merged.iter() {
            if !s.contains(&u.speaker) {
                s.push(u.speaker.clone());
            }
        }
        if s.is_empty() {
            s = speakers_info.iter().map(|x| x.name.clone()).collect();
        }
        s
    };

    let stem = audio_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "meeting".to_string());
    let out_dir = audio_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let method = if is_stereo {
        "Микрофон и система (модели: VAD, GigaAM, векторы голосов)".to_string()
    } else {
        speaker_method.to_string() + " + разделение голосов"
    };
    let meta = serde_json::json!({
        "audio": audio_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        "is_video": video,
        "duration": report::human_duration(duration_sec),
        "engine": engine_name,
        "speaker_method": method,
        "speakers": speakers,
        "speakers_info": speakers_info.iter().map(report::speaker_to_json).collect::<Vec<_>>(),
    });
    let md_path = out_dir.join(format!("{stem}.md"));
    let txt_path = out_dir.join(format!("{stem}.txt"));
    let json_path = out_dir.join(format!("{stem}.transcript.json"));
    std::fs::write(&md_path, report::build_markdown(&meta, &merged)).map_err(|e| e.to_string())?;
    std::fs::write(&txt_path, report::build_text(&merged)).map_err(|e| e.to_string())?;
    let doc = serde_json::json!({
        "meta": meta,
        "stats": {"segments": merged.len(), "words": words, "speech_sec": (speech_sec*10.0).round()/10.0, "diarized": true},
        "utterances": merged.iter().map(|u| serde_json::json!({
            "start": u.start, "end": u.end, "speaker": u.speaker,
            "channel": u.channel, "text": u.text, "diarized": u.diarized,
            "silhouette": u.silhouette,
        })).collect::<Vec<_>>(),
    });
    std::fs::write(&json_path, serde_json::to_string_pretty(&doc).unwrap())
        .map_err(|e| e.to_string())?;
    models.embed = embed_ext;
    models.segmentation = segmenter;
    return_cached_models(models);
    report::record_stats(duration_sec);
    Ok(TranscribeReport {
        md_path,
        txt_path,
        json_path,
        utterances: merged.len(),
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
    use super::{looks_like_asr_loop, quiet_blank_penalty};

    #[test]
    fn detects_repeated_asr_hallucination() {
        let looped = format!(
            "Проверка того, как пишутся{} пишется либо.",
            "-и".repeat(40)
        );
        assert!(looks_like_asr_loop(&looped));
        assert!(looks_like_asr_loop(
            "кстати, кстати, кстати, кстати, кстати, кстати, кстати, кстати, всё"
        ));
        assert!(looks_like_asr_loop(
            "Да-да-да-да-да-да-да-да-да-да-да-да, вот так вот оно и было"
        ));
    }

    #[test]
    fn blank_penalty_only_for_short_quiet_speech() {
        assert_eq!(quiet_blank_penalty(2.6, 0.011), 2.0);
        assert_eq!(quiet_blank_penalty(10.0, 0.011), 0.0);
        assert_eq!(quiet_blank_penalty(2.6, 0.0003), 0.0);
        assert_eq!(quiet_blank_penalty(2.6, 0.2), 0.0);
    }

    #[test]
    fn keeps_normal_speech_with_small_repetitions() {
        assert!(!looks_like_asr_loop(
            "Проверка того, как пишется, пишется ли."
        ));
        assert!(!looks_like_asr_loop(
            "Да-да-да, хорошо, встретимся завтра в десять утра."
        ));

        assert!(!looks_like_asr_loop(""));
    }
}
