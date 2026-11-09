use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use mr_audio::decode::decode_to_channels_16k_auto;
use mr_ml::onnx::ctc::GigaCTC;
use mr_ml::onnx::embed::EmbeddingExtractor;
use mr_ml::onnx::rnnt::GigaAM;
use mr_ml::onnx::segmentation::SegmentationModel;
use mr_ml::onnx::vad::SileroVad;
use mr_ml::onnx::whisper::Whisper;

use super::asr_engine::{models_root, AsrEngine, LoadPlan, CTC_DIR, GIGAAM_DIR, WHISPER_DIR};
use super::memory;

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
static MODELS_CACHE: OnceLock<Mutex<Option<(ModelSet, LoadPlan)>>> = OnceLock::new();
/// Сброс кэша бесполезен без поколения: активная расшифровка вернула бы старые веса.
static MODELS_GEN: AtomicU64 = AtomicU64::new(0);

fn take_cached_models(plan: LoadPlan) -> Result<(ModelSet, u64), String> {
    let cache = MODELS_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    let reusable = guard
        .as_ref()
        .is_some_and(|(_, cached)| cached.covers(plan));
    if !reusable {
        *guard = Some((ModelSet::load(plan)?, plan));
    }
    Ok((
        guard.take().expect("только что положили").0,
        MODELS_GEN.load(Ordering::SeqCst),
    ))
}

fn return_cached_models(models: ModelSet, gen: u64, plan: LoadPlan) {
    if MODELS_GEN.load(Ordering::SeqCst) != gen {
        return;
    }
    let cache = MODELS_CACHE.get_or_init(|| Mutex::new(None));
    *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((models, plan));
}

/// Модели возвращаются в кэш на любом пути выхода: ошибка распознавания не
/// должна выбрасывать сотни мегабайт весов, уже прочитанных с диска.
pub(super) struct ModelsGuard {
    models: Option<ModelSet>,
    gen: u64,
    plan: LoadPlan,
}

impl ModelsGuard {
    pub(super) fn take(asr: Option<AsrEngine>, ctc_enabled: bool) -> Result<Self, String> {
        let plan = LoadPlan::new(asr, ctc_enabled);
        let (models, gen) = take_cached_models(plan)?;
        Ok(Self {
            models: Some(models),
            gen,
            plan,
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
            return_cached_models(models, self.gen, self.plan);
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
    let mut models = ModelsGuard::take(None, false)?;
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
    fn load(plan: LoadPlan) -> Result<Self, String> {
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
        let gigaam = match plan.asr {
            Some(AsrEngine::Gigaam) => {
                let dir = models_root().join(GIGAAM_DIR);
                match GigaAM::load(&dir) {
                    Ok(model) => Some(model),
                    Err(error) => {
                        log::error!(
                            target: "model",
                            id = GIGAAM_DIR,
                            dir = dir.display().to_string(),
                            error = error;
                            "ev=model.load_failed"
                        );
                        return Err(AsrEngine::Gigaam.missing_error());
                    }
                }
            }
            _ => None,
        };
        let ctc = if plan.ctc {
            let dir = models_root().join(CTC_DIR);
            match GigaCTC::load(&dir) {
                Ok(model) => Some(model),
                Err(error) => {
                    log::warn!(
                        target: "model",
                        id = CTC_DIR,
                        dir = dir.display().to_string(),
                        error = error;
                        "ev=model.load_failed"
                    );
                    None
                }
            }
        } else {
            None
        };
        let whisper = match plan.asr {
            Some(AsrEngine::Whisper) => {
                let dir = models_root().join(WHISPER_DIR);
                match Whisper::load(&dir) {
                    Ok(model) => Some(model),
                    Err(error) => {
                        log::error!(
                            target: "model",
                            id = WHISPER_DIR,
                            dir = dir.display().to_string(),
                            error = error;
                            "ev=model.load_failed"
                        );
                        return Err(AsrEngine::Whisper.missing_error());
                    }
                }
            }
            _ => None,
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
