mod channel_mix;
pub mod diarize;
pub mod document;
pub mod events;
pub mod memory;
pub mod report;
mod speaker_assignment;
mod word_speakers;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use document::{read_transcript, unique_speech_sec, validate, Meta, Stats, TranscriptDoc};

use mr_audio::decode::{decode_to_channels_16k_auto, is_video_path};
use mr_packs as packs;

use mr_ml::hotwords::{sanitize_hotwords, DEFAULT_CONFIDENCE_THRESHOLD};
use mr_ml::onnx::ctc::{align_to_reference_timed, GigaCTC};
use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::rnnt::GigaAM;
use mr_ml::onnx::segmentation::SegmentationModel;
use mr_ml::onnx::vad::{vad_segments_from_probs, SileroVad, VadRunStats, VAD_WINDOW};
use mr_ml::onnx::whisper::Whisper;

pub use events::{
    clear_queue, emit_model_event, enqueue_transcription, finish_model_events, get_queue_status,
    has_pending_work, is_transcribe_running, remove_from_queue, rename_in_queue,
    spawn_transcription_worker, start_model_download, take_new_events, transcribing_file,
};

fn models_root() -> PathBuf {
    packs::default_app_data_dir().join("models")
}

fn find_model_file(model_id: &str, needle: &str) -> Option<PathBuf> {
    let dir = models_root().join(model_id);
    let mut files = 0usize;
    let mut found = None;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            files += 1;
            if found.is_none()
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(needle))
            {
                found = Some(path);
            }
        }
    }
    log::debug!(
        target: "model",
        id = model_id,
        dir = dir.display().to_string(),
        files = files,
        found = found
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        "ev=model.search"
    );
    found
}

pub struct ModelSet {
    vad: SileroVad,
    gigaam: Option<GigaAM>,
    ctc: Option<GigaCTC>,
    whisper: Option<Whisper>,
    embed: Option<EmbeddingExtractor>,
    segmentation: Option<SegmentationModel>,
}

/// Кэш моделей процесса: веса в сотни МБ читаются с диска один раз,
/// а не на каждую расшифровку. Конкуренция исключена флагом RUNNING.
static MODELS_CACHE: OnceLock<Mutex<Option<ModelSet>>> = OnceLock::new();
/// Сброс кэша бесполезен без поколения: активная расшифровка вернула бы старые веса.
static MODELS_GEN: AtomicU64 = AtomicU64::new(0);

fn take_cached_models() -> Result<(ModelSet, u64), String> {
    let cache = MODELS_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(ModelSet::load()?);
    }
    Ok((
        guard.take().expect("только что положили"),
        MODELS_GEN.load(Ordering::SeqCst),
    ))
}

fn return_cached_models(models: ModelSet, gen: u64) {
    if MODELS_GEN.load(Ordering::SeqCst) != gen {
        return;
    }
    let cache = MODELS_CACHE.get_or_init(|| Mutex::new(None));
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some(models);
}

/// Модели возвращаются в кэш на любом пути выхода: ошибка распознавания не
/// должна выбрасывать сотни мегабайт весов, уже прочитанных с диска.
struct ModelsGuard {
    models: Option<ModelSet>,
    gen: u64,
}

impl ModelsGuard {
    fn take() -> Result<Self, String> {
        let (models, gen) = take_cached_models()?;
        Ok(Self {
            models: Some(models),
            gen,
        })
    }
}

impl std::ops::Deref for ModelsGuard {
    type Target = ModelSet;

    fn deref(&self) -> &Self::Target {
        self.models.as_ref().expect("модели изъяты только в Drop")
    }
}

impl std::ops::DerefMut for ModelsGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.models.as_mut().expect("модели изъяты только в Drop")
    }
}

impl Drop for ModelsGuard {
    fn drop(&mut self) {
        if let Some(models) = self.models.take() {
            return_cached_models(models, self.gen);
        }
    }
}

pub fn invalidate_models_cache() {
    MODELS_GEN.fetch_add(1, Ordering::SeqCst);
    let cache = MODELS_CACHE.get_or_init(|| Mutex::new(None));
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn extract_utterance_embedding(
    audio: &Path,
    start: f64,
    end: f64,
    channel: Option<i32>,
) -> Result<(Vec<f64>, f64, f64), String> {
    memory::check(audio)?;
    if end - start < 0.4 {
        return Err("Фрагмент короче 0.4 с: голос не извлечь".to_string());
    }
    if !mr_audio::has_audio_track(audio) {
        return Err("Файл не содержит аудиодорожки".to_string());
    }
    let snippet = audio
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        .then(|| {
            mr_audio::decode::decode_wav_range_16k(
                audio,
                start,
                end,
                channel.map(|c| c.max(0) as usize),
            )
            .ok()
        })
        .flatten();
    let samples: Vec<f32> = match snippet {
        Some(samples) => samples,
        None => {
            let channels = decode_to_channels_16k_auto(audio).map_err(|e| e.to_string())?;
            let ch = channel.unwrap_or(0).max(0) as usize;
            let samples = channels
                .get(ch)
                .or(channels.first())
                .ok_or_else(|| "Файл не содержит аудиодорожки".to_string())?;
            let lo = ((start.max(0.0) * 16000.0) as usize).min(samples.len());
            let hi = ((end.max(start).max(0.0) * 16000.0) as usize).min(samples.len());
            samples[lo..hi].to_vec()
        }
    };
    if samples.len() < (16000.0 * 0.4) as usize {
        return Err("Фрагмент короче 0.4 с: голос не извлечь".to_string());
    }
    let mut models = ModelsGuard::take()?;
    let mut clean_samples = samples;
    if let Some(segmentation) = models.segmentation.as_mut() {
        if let Ok(seg) = segmentation.segment(&clean_samples) {
            let clean_len = seg.exclude_overlap(&mut clean_samples, 0);
            clean_samples.truncate(clean_len);
        }
    }
    if clean_samples.len() < (16000.0 * 0.4) as usize {
        return Err(
            "Во фрагменте одновременная речь или мало чистого голоса — выберите другой участок"
                .to_string(),
        );
    }
    let emb = models
        .embed
        .as_mut()
        .ok_or_else(|| {
            "Нет модели голосовых отпечатков. Скачайте её во вкладке «Компоненты»".to_string()
        })?
        .embed(&clean_samples)?;
    let emb = emb.ok_or_else(|| "Голос не извлечь: тишина или шум".to_string())?;
    let clean_dur = clean_samples.len() as f64 / 16000.0;
    let rms = (clean_samples.iter().map(|&x| (x * x) as f64).sum::<f64>()
        / clean_samples.len().max(1) as f64)
        .sqrt();
    let quality =
        ((rms / 0.05).clamp(0.3, 1.2) * (clean_dur / (end - start).max(0.4)).min(1.0) * 1000.0)
            .round()
            / 1000.0;
    Ok((emb, clean_dur, quality))
}

impl ModelSet {
    pub fn load() -> Result<Self, String> {
        let vad_path = find_model_file("vad-silero-v6", ".onnx")
            .ok_or_else(|| "Модель Silero VAD не скачана. Откройте «Настройки» → «Модели ИИ» и нажмите «Скачать».".to_string())?;
        let vad = SileroVad::load(&vad_path).map_err(|error| {
            log::error!(
                target: "model",
                id = "vad-silero-v6",
                file = vad_path.display().to_string(),
                error = error;
                "ev=model.load_failed"
            );
            error
        })?;
        let giga_dir = models_root().join("asr-gigaam-v3-rnnt-int8");
        let gigaam = match GigaAM::load(&giga_dir) {
            Ok(model) => Some(model),
            Err(error) => {
                log::warn!(
                    target: "model",
                    id = "asr-gigaam-v3-rnnt-int8",
                    dir = giga_dir.display().to_string(),
                    error = error;
                    "ev=model.load_failed"
                );
                None
            }
        };
        let ctc_dir = models_root().join("asr-gigaam-v3-e2e-ctc-int8");
        let ctc = match GigaCTC::load(&ctc_dir) {
            Ok(model) => Some(model),
            Err(error) => {
                log::warn!(
                    target: "model",
                    id = "asr-gigaam-v3-e2e-ctc-int8",
                    dir = ctc_dir.display().to_string(),
                    error = error;
                    "ev=model.load_failed"
                );
                None
            }
        };
        let whisper_dir = models_root().join("asr-whisper-large-v3-turbo-q");
        let whisper = match Whisper::load(&whisper_dir) {
            Ok(model) => Some(model),
            Err(error) => {
                log::warn!(
                    target: "model",
                    id = "asr-whisper-large-v3-turbo-q",
                    dir = whisper_dir.display().to_string(),
                    error = error;
                    "ev=model.load_failed"
                );
                None
            }
        };
        let emb_path = find_model_file("diar-eres2netv2-emb", ".onnx")
            .or_else(|| find_model_file("diar-pyannote-emb", ".onnx"))
            .or_else(|| find_model_file("diar-wespeaker-resnet34", ".onnx"));
        let embed = emb_path.and_then(|path| match EmbeddingExtractor::load(&path) {
            Ok(model) => Some(model),
            Err(error) => {
                log::warn!(
                    target: "model",
                    id = "diar-embed",
                    file = path.display().to_string(),
                    error = error;
                    "ev=model.load_failed"
                );
                None
            }
        });
        let segmentation =
            find_model_file("diar-pyannote-segmentation-3", ".onnx").and_then(|path| {
                match SegmentationModel::load(&path) {
                    Ok(model) => Some(model),
                    Err(error) => {
                        log::warn!(
                            target: "model",
                            id = "diar-pyannote-segmentation-3",
                            file = path.display().to_string(),
                            error = error;
                            "ev=model.load_failed"
                        );
                        None
                    }
                }
            });
        if gigaam.is_none() && whisper.is_none() {
            log::error!(target: "model", reason = "no_asr_model"; "ev=model.load_failed");
            return Err("Нет модели распознавания речи. Скачайте GigaAM или Whisper: «Настройки» → «Модели ИИ».".to_string());
        }
        Ok(Self {
            vad,
            gigaam,
            ctc,
            whisper,
            embed,
            segmentation,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WordConf {
    pub w: String,
    pub conf: f64,
    pub needs_review: bool,
    #[serde(default)]
    pub start_sec: f64,
    #[serde(default)]
    pub end_sec: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Utterance {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub channel: Option<i32>,
    pub text: String,
    #[serde(default)]
    pub diarized: bool,
    #[serde(default)]
    pub silhouette: f64,
    #[serde(default)]
    pub words: Vec<WordConf>,
    #[serde(default)]
    pub ctc_used: bool,
}

#[derive(Debug, Clone)]
pub struct SpeakerInfo {
    pub id: String,
    pub name: String,
    pub channel: Option<i32>,
    pub matched: bool,
    pub matched_name: Option<String>,
    pub matched_pack_id: Option<String>,
    pub matched_pack_name: Option<String>,
    pub similarity: f64,
    pub silhouette: f64,
    pub candidates: Vec<(String, f64)>,
    pub embedding: Option<Vec<f64>>,
    pub samples: Vec<Sample>,
    pub utterances_count: usize,
    pub total_speech_sec: f64,
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub start: f64,
    pub end: f64,
    pub dur: f64,
    pub text: String,
    pub full_text: String,
    pub channel: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct TranscribeOptions {
    pub speaker_left: String,
    pub speaker_right: String,
    pub engine: String,
    /// Язык Whisper в виде кода («ru», «en»…); «auto» — определение моделью.
    /// GigaAM игнорирует поле (он одноязычный, русский).
    pub language: String,
    pub min_speakers: usize,
    pub max_speakers: usize,
    pub hotwords: Vec<String>,
    pub ctc_enabled: bool,
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
            hotwords: Vec::new(),
            ctc_enabled: true,
        }
    }
}

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

const MAX_SEGMENT_SEC: f32 = 20.0;
const SEGMENT_GAP_SEC: f32 = 0.5;
const SEGMENT_PAD_SEC: f64 = 0.25;
const VAD_THRESHOLD: f32 = 0.25;
const VAD_MIN_SPEECH_SEC: f32 = 0.25;
const VAD_MIN_SILENCE_SEC: f32 = 0.5;

#[derive(Debug, Clone, Copy, Default)]
struct AudioStats {
    rms: f64,
    peak: f32,
    clip_pct: f64,
    silence_pct: f64,
    nan: usize,
    inf: usize,
}

fn stage_begin(name: &'static str) {
    if !cfg!(debug_assertions) {
        return;
    }
    crate::logging::enter_stage(name);
    log::debug!(target: "run", "ev=stage.start");
}

fn stage_end(_name: &'static str, started: Instant) {
    if !cfg!(debug_assertions) {
        return;
    }
    log::debug!(
        target: "run",
        elapsed_ms = started.elapsed().as_millis() as u64;
        "ev=stage.end"
    );
}

fn audio_stats(samples: &[f32]) -> AudioStats {
    if samples.is_empty() {
        return AudioStats::default();
    }
    let mut sum_sq = 0.0f64;
    let mut peak = 0.0f32;
    let mut clipped = 0usize;
    let mut silent = 0usize;
    let mut nan = 0usize;
    let mut inf = 0usize;
    for &sample in samples {
        if sample.is_nan() {
            nan += 1;
            continue;
        }
        if sample.is_infinite() {
            inf += 1;
            continue;
        }
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_sq += f64::from(sample) * f64::from(sample);
        if abs >= 0.999 {
            clipped += 1;
        }
        if abs < 1e-4 {
            silent += 1;
        }
    }
    let finite = samples.len().saturating_sub(nan + inf).max(1);
    AudioStats {
        rms: (sum_sq / finite as f64).sqrt(),
        peak,
        clip_pct: clipped as f64 * 100.0 / finite as f64,
        silence_pct: silent as f64 * 100.0 / finite as f64,
        nan,
        inf,
    }
}

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f32 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn finite_probs(probs: &[f32]) -> Vec<f32> {
    let mut values: Vec<f32> = probs.iter().copied().filter(|p| p.is_finite()).collect();
    values.sort_by(f32::total_cmp);
    values
}

fn log_vad_summary(ch: i32, probs: &[f32], stats: VadRunStats) -> Vec<(f32, f32)> {
    let segments = vad_segments_from_probs(
        probs,
        VAD_THRESHOLD,
        VAD_MIN_SPEECH_SEC,
        VAD_MIN_SILENCE_SEC,
    );
    if !cfg!(debug_assertions) {
        return segments;
    }
    let speech_win = probs.iter().filter(|&&p| p >= VAD_THRESHOLD).count();
    let speech_sec: f32 = segments.iter().map(|(a, b)| b - a).sum::<f32>().max(0.0);
    let max_seg_sec = segments.iter().map(|(a, b)| b - a).fold(0.0f32, f32::max);
    let sorted = finite_probs(probs);
    log::info!(
        target: "vad",
        ch = ch,
        windows = stats.windows,
        speech_win = speech_win,
        speech_pct = if stats.windows == 0 { 0.0 } else { speech_win as f64 * 100.0 / stats.windows as f64 },
        silent_skip = stats.silent_skip,
        silent_infer = stats.silent_infer,
        resets = stats.resets,
        non_finite = stats.non_finite,
        p_min = percentile(&sorted, 0.0),
        p50 = percentile(&sorted, 0.5),
        p95 = percentile(&sorted, 0.95),
        p99 = percentile(&sorted, 0.99),
        p_max = percentile(&sorted, 1.0),
        segs = segments.len(),
        speech_sec = speech_sec,
        max_seg_sec = max_seg_sec;
        "ev=vad.summary"
    );
    let mut buckets = [0usize; 20];
    for &probability in probs {
        if probability.is_finite() {
            let index = ((probability.clamp(0.0, 1.0) * 19.0).round() as usize).min(19);
            buckets[index] += 1;
        }
    }
    log::debug!(
        target: "vad",
        ch = ch,
        b0 = buckets[0], b1 = buckets[1], b2 = buckets[2], b3 = buckets[3],
        b4 = buckets[4], b5 = buckets[5], b6 = buckets[6], b7 = buckets[7],
        b8 = buckets[8], b9 = buckets[9], b10 = buckets[10], b11 = buckets[11],
        b12 = buckets[12], b13 = buckets[13], b14 = buckets[14], b15 = buckets[15],
        b16 = buckets[16], b17 = buckets[17], b18 = buckets[18], b19 = buckets[19];
        "ev=vad.hist"
    );
    segments
}

fn log_vad_segments(ch: i32, probs: &[f32], segments: &[(f32, f32)]) {
    if !log::log_enabled!(target: "vad", log::Level::Debug) {
        return;
    }
    let win_sec = VAD_WINDOW as f32 / 16000.0;
    for (index, &(start, end)) in segments.iter().enumerate() {
        let lo = (start / win_sec) as usize;
        let hi = ((end / win_sec).ceil() as usize).min(probs.len());
        let slice = probs.get(lo..hi).unwrap_or(&[]);
        let mean = if slice.is_empty() {
            0.0
        } else {
            slice.iter().map(|&p| p as f64).sum::<f64>() / slice.len() as f64
        };
        let max = slice.iter().copied().fold(0.0f32, f32::max);
        log::debug!(
            target: "vad",
            ch = ch,
            i = index,
            start_sec = start,
            end_sec = end,
            dur_sec = end - start,
            p_mean = mean,
            p_max = max;
            "ev=vad.segment"
        );
    }
}

fn log_vad_windows(ch: i32, samples: &[f32], probs: &[f32]) {
    if !log::log_enabled!(target: "vad", log::Level::Trace) {
        return;
    }
    let mut emitted = 0usize;
    let mut suppressed = 0usize;
    for (index, &probability) in probs.iter().enumerate() {
        let interesting = (0.05..=0.6).contains(&probability);
        if index % 64 != 0 && !interesting {
            suppressed += 1;
            continue;
        }
        if emitted >= 2000 {
            suppressed += 1;
            continue;
        }
        let lo = index * VAD_WINDOW;
        let hi = (lo + VAD_WINDOW).min(samples.len());
        let stats = audio_stats(&samples[lo..hi]);
        log::trace!(
            target: "vad",
            ch = ch,
            i = index,
            t_sec = index as f64 * VAD_WINDOW as f64 / 16000.0,
            p = probability,
            rms = stats.rms,
            peak = stats.peak,
            action = if probability >= VAD_THRESHOLD { "speech" } else { "silence" };
            "ev=vad.window"
        );
        emitted += 1;
    }
    if suppressed > 0 {
        log::debug!(
            target: "vad",
            ch = ch,
            count = suppressed;
            "ev=vad.window.suppressed"
        );
    }
}

fn mel_stats(mel: &[Vec<f32>]) -> (f32, f32, usize) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut non_finite = 0usize;
    for row in mel {
        for &value in row {
            if !value.is_finite() {
                non_finite += 1;
                continue;
            }
            min = min.min(value);
            max = max.max(value);
        }
    }
    if min.is_finite() {
        (min, max, non_finite)
    } else {
        (0.0, 0.0, non_finite)
    }
}

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

#[derive(Debug, Clone, Copy, Default)]
struct LoopMetrics {
    words: usize,
    max_run: usize,
    unique_ratio: f32,
    max_count: usize,
}

/// Признак зацикливания ASR: один и тот же токен/слог повторяется десятки раз
/// или текст почти полностью состоит из нескольких слов. Такой фрагмент —
/// галлюцинация декодера на шуме, а не речь; в отчёт он не попадает.
fn asr_loop_metrics(text: &str) -> LoopMetrics {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        return LoopMetrics::default();
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
    LoopMetrics {
        words: words.len(),
        max_run,
        unique_ratio: counts.len() as f32 / words.len() as f32,
        max_count: counts.values().copied().max().unwrap_or(0),
    }
}

fn metrics_look_like_loop(metrics: LoopMetrics) -> bool {
    if metrics.words < 8 {
        return false;
    }
    metrics.max_run >= 8
        || (metrics.max_run >= 6 && metrics.unique_ratio < 0.4)
        || (metrics.max_count >= 10 && metrics.max_count as f32 / metrics.words as f32 >= 0.35)
        || (metrics.words >= 20 && metrics.unique_ratio < 0.22)
}

#[cfg(test)]
fn looks_like_asr_loop(text: &str) -> bool {
    metrics_look_like_loop(asr_loop_metrics(text))
}

fn model_files(model_id: &str) -> Vec<serde_json::Value> {
    let dir = models_root().join(model_id);
    let mut files: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            Some(serde_json::json!({
                "id": model_id,
                "file": path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                "bytes": entry.metadata().map(|m| m.len()).unwrap_or(0),
            }))
        })
        .collect();
    files.sort_by(|a, b| {
        a["file"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["file"].as_str().unwrap_or_default())
    });
    files
}

fn write_run_manifest(
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

fn write_transcript_doc(path: &Path, doc: &TranscriptDoc) -> Result<(), String> {
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

pub fn transcribe_file(
    audio_path: &Path,
    opts: &TranscribeOptions,
    on_event: &dyn Fn(u8, &str),
) -> Result<TranscribeReport, String> {
    let run_id = crate::logging::begin_run();
    let started = Instant::now();
    log::info!(
        target: "run",
        run = run_id.clone(),
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
                run = run_id,
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
                run = run_id,
                stage = stage.clone(),
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

    // Каналы отдаём как есть: `channels` больше нигде не нужен, поэтому
    // раскладываем его по индексам без копий (иначе длинная запись занимала
    // вдвое больше памяти).
    let channel_samples: HashMap<usize, Vec<f32>> = channels
        .into_iter()
        .enumerate()
        .filter(|(i, _)| chan_list.iter().any(|(ch, _)| ch == i))
        .collect();

    let mut channel_grouped: HashMap<usize, Vec<(f64, f64)>> = HashMap::new();
    let mut channel_vad: HashMap<usize, Vec<f32>> = HashMap::new();
    let mut channel_vad_stats: HashMap<usize, VadRunStats> = HashMap::new();
    let channel_count = chan_list.len().max(1) as f64;
    let vad_stage = Instant::now();
    stage_begin("vad");
    for (ch_i, (ch, label)) in chan_list.iter().enumerate() {
        let samples = &channel_samples[ch];
        let base = 6.0 + 8.0 * ch_i as f64 / channel_count;
        let span = 8.0 / channel_count;
        let windows = samples.len().div_ceil(VAD_WINDOW);
        log::debug!(
            target: "vad",
            ch = *ch as i64,
            samples = samples.len(),
            windows = windows,
            threshold = VAD_THRESHOLD,
            min_speech_sec = VAD_MIN_SPEECH_SEC,
            min_silence_sec = VAD_MIN_SILENCE_SEC;
            "ev=vad.start"
        );
        let (probs, stats) = models
            .vad
            .process_all_with_stats(samples, |done, total| {
                let pct = (base + span * done as f64 / total.max(1) as f64) as u8;
                on_event(
                    pct.clamp(6, 14),
                    &format!("Поиск речи ({label}): {done} из {total} окон"),
                );
            })
            .map_err(|e| format!("VAD: {e}"))?;
        channel_vad_stats.insert(*ch, stats);
        channel_vad.insert(*ch, probs);
    }
    if is_stereo {
        if let (Some(mic), Some(system)) = (channel_samples.get(&0), channel_samples.get(&1)) {
            if let Some(probs) = channel_vad.get_mut(&0) {
                let stats = channel_mix::suppress_mic_bleed_with_stats(mic, system, probs);
                log::info!(
                    target: "audio",
                    ch = 0,
                    delay = stats.delay,
                    prom = stats.prominence,
                    suppressed = stats.suppressed,
                    total = stats.frames,
                    probe_sec = stats.probe_sec;
                    "ev=audio.bleed"
                );
                if stats.suppressed > 0 {
                    on_event(
                        14,
                        &format!("Микрофон: подавлено эхо в {} кадрах", stats.suppressed),
                    );
                }
            }
        }
    }
    for (ch_i, (ch, label)) in chan_list.iter().enumerate() {
        let samples = &channel_samples[ch];
        let base = 6.0 + 8.0 * ch_i as f64 / channel_count;
        let span = 8.0 / channel_count;
        let probs = &channel_vad[ch];
        let stats = channel_vad_stats.get(ch).copied().unwrap_or_default();
        let segs = log_vad_summary(*ch as i32, probs, stats);
        log_vad_windows(*ch as i32, samples, probs);
        log_vad_segments(*ch as i32, probs, &segs);
        let grouped = diarize::merge_quiet_short(
            samples,
            diarize::group_segments(&segs, probs, MAX_SEGMENT_SEC, SEGMENT_GAP_SEC, *ch as i32),
            *ch as i32,
        );
        if cfg!(debug_assertions) && crate::logging::dump_enabled() {
            let channel = format!("ch{ch}");
            crate::logging::write_artifact_f32(&format!("vad/{channel}.probs.f32"), probs);
            crate::logging::write_artifact_json(
                &format!("vad/{channel}.segments.json"),
                &serde_json::json!({
                    "raw": segs,
                    "grouped": grouped,
                }),
            );
        }
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
    stage_end("vad", vad_stage);
    let total_segs: usize = channel_grouped.values().map(|v| v.len()).sum();
    let mut done_segs = 0usize;
    let mut seg_index = 0usize;
    let mut ctc_used_any = false;
    let mut channel_utts: HashMap<usize, Vec<Utterance>> = HashMap::new();
    let asr_stage = Instant::now();
    stage_begin("asr");
    for (ch, label) in chan_list.iter() {
        let samples = &channel_samples[ch];
        let mut utts = vec![];
        for (s, e) in channel_grouped[ch].iter() {
            let a = (s - SEGMENT_PAD_SEC).max(0.0);
            let b = (e + SEGMENT_PAD_SEC).min(samples.len() as f64 / 16000.0);
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
            let chunk = &samples[lo..hi];
            let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
            // Длительность — до padding-а, иначе короткая реплика у границы
            // 4 секунд теряет штраф из-за добавленных 0.5 с.
            let blank_penalty = if use_gigaam {
                quiet_blank_penalty(*e - *s, rms)
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
            // Зацикленный вывод жадного декодера на шуме — не речь.
            if !text.is_empty() && !looped {
                let mut words: Vec<WordConf> = Vec::new();
                let mut ctc_used = false;
                let mut final_text = text;
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
                                        DEFAULT_CONFIDENCE_THRESHOLD,
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
                let words_count = final_text.split_whitespace().count();
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
                    chars = text.chars().count(),
                    words = metrics.words,
                    text_preview = crate::logging::text_preview(&text),
                    loop = looped;
                    "ev=asr.seg.done"
                );
            }
            seg_index += 1;
        }
        diarize::move_boundary_words(&mut utts);
        channel_utts.insert(*ch, utts);
    }
    stage_end("asr", asr_stage);

    let diar_stage = Instant::now();
    stage_begin("diar");
    let profiles = diarize::load_profiles();
    // Заимствуем модели на время диаризации: guard вернёт их в кэш на выходе.
    let mut speakers_info: Vec<SpeakerInfo> = vec![];
    let mut all_utts: Vec<Utterance> = vec![];
    for (ch, _label) in chan_list.iter() {
        let samples = &channel_samples[ch];
        let source_utts = channel_utts.remove(ch).unwrap_or_default();
        let segmentation_started = Instant::now();
        let segmentation = if models.embed.is_some() && !source_utts.is_empty() {
            on_event(80, "Ищу одновременную речь…");
            models.segmentation.as_mut().and_then(|model| {
                let result = match channel_vad.get(ch) {
                    Some(probs) => model.segment_with_vad(samples, probs),
                    None => model.segment(samples),
                };
                match result {
                    Ok(result) => {
                        let overlap_frames =
                            result.frames.iter().filter(|&&class| class >= 4).count();
                        log::debug!(
                            target: "diar",
                            ch = *ch as i64,
                            windows = result.windows.len(),
                            frames = result.frames.len(),
                            overlap_frames = overlap_frames,
                            elapsed_ms = segmentation_started.elapsed().as_millis() as u64;
                            "ev=diar.segmentation"
                        );
                        Some(result)
                    }
                    Err(error) => {
                        log::warn!(
                            target: "diar",
                            ch = *ch as i64,
                            reason = "segmentation_failed",
                            error = error.to_string();
                            "ev=diar.profile.load"
                        );
                        crate::note!("PyAnnote segmentation недоступна: {error}");
                        None
                    }
                }
            })
        } else {
            None
        };
        // В стерео каналы уже разделены по голосам, а дробление по паузам
        // даёт лишние короткие эмбеддинги и перекластеризацию.
        let mut diar_utts = if is_stereo || mixed_channels.is_some() {
            source_utts.clone()
        } else {
            on_event(80, "Делю речь на паузы для спикеров…");
            diarize::split_at_pauses(samples, &source_utts)
        };
        on_event(80, "Ищу смену голоса внутри реплик…");
        diar_utts = diarize::split_long_by_speaker_change(
            samples,
            diar_utts,
            &mut models.embed,
            segmentation.as_ref(),
            &|pct, msg| on_event(pct, msg),
        );
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
            &mut models.embed,
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
    stage_end("diar", diar_stage);
    all_utts.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.speaker.cmp(&b.speaker)));
    let merged = diarize::merge_utterances(all_utts);
    diarize::refresh_speaker_stats(&mut speakers_info, &merged);
    if cfg!(debug_assertions) {
        let speaker_summary: Vec<serde_json::Value> = speakers_info
            .iter()
            .map(|info| {
                let mut value = serde_json::json!({
                    "id": crate::logging::speaker_token(&info.name),
                    "utts": info.utterances_count,
                    "speech_sec": info.total_speech_sec,
                    "silhouette": info.silhouette,
                    "similarity": info.similarity,
                    "matched": info.matched,
                    "candidates": info.candidates.iter().map(|(name, score)| {
                        serde_json::json!({
                            "name": crate::logging::speaker_token(name),
                            "similarity": score,
                        })
                    }).collect::<Vec<_>>(),
                });
                if crate::logging::dump_full() {
                    value["embedding"] = serde_json::json!(info.embedding);
                }
                value
            })
            .collect();
        crate::logging::write_artifact_json(
            "diar/summary.json",
            &serde_json::json!({"speakers": speaker_summary}),
        );
    }
    let words = merged
        .iter()
        .map(|u| u.text.split_whitespace().count())
        .sum::<usize>();
    let speech_sec = unique_speech_sec(&merged);

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
        schema: document::SCHEMA_VERSION,
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
    use super::{looks_like_asr_loop, quiet_blank_penalty, Utterance};

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

    #[test]
    fn old_transcript_without_words_parses() {
        // P9: старые .transcript.json без words/ctc_used читаются с дефолтами.
        let u: Utterance = serde_json::from_value(serde_json::json!({
            "start": 0.0, "end": 1.0, "speaker": "Спикер",
            "channel": null, "text": "раз два"
        }))
        .unwrap();
        assert!(u.words.is_empty());
        assert!(!u.ctc_used);
    }
}
