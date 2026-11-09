use std::path::{Path, PathBuf};

use mr_packs as packs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use mr_audio::decode::decode_to_channels_16k_auto;
use mr_ml::onnx::ctc::GigaCTC;
use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::rnnt::GigaAM;
use mr_ml::onnx::segmentation::SegmentationModel;
use mr_ml::onnx::vad::SileroVad;
use mr_ml::onnx::whisper::Whisper;

use super::memory;

pub(super) fn models_root() -> PathBuf {
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

pub(super) struct ModelSet {
    pub(super) vad: SileroVad,
    pub(super) gigaam: Option<GigaAM>,
    pub(super) ctc: Option<GigaCTC>,
    pub(super) whisper: Option<Whisper>,
    pub(super) embed: Option<EmbeddingExtractor>,
    pub(super) segmentation: Option<SegmentationModel>,
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
pub(super) struct ModelsGuard {
    models: Option<ModelSet>,
    gen: u64,
}

impl ModelsGuard {
    pub(super) fn take() -> Result<Self, String> {
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
    fn load() -> Result<Self, String> {
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
