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

/// Резолв по `files[]` вшитого манифеста в порядке массива: посторонний файл в
/// каталоге модели (остаток докачки, ручная копия) не подменяет проверенные веса.
fn find_model_file(model_id: &str, needle: &str) -> Option<PathBuf> {
    let dir = models_root().join(model_id);
    let found = match mr_models::bundled()
        .models
        .iter()
        .find(|m| m.id == model_id)
    {
        Some(entry) => entry
            .files
            .iter()
            .map(|f| f.filename.as_str())
            .filter(|name| !is_temp_name(name))
            .map(|name| dir.join(name))
            .find(|path| path.is_file() && name_matches(path, needle)),
        // Идентификаторы прошлых выпусков живут вне манифеста: для них остаётся
        // поиск по каталогу.
        None => scan_dir(&dir, needle),
    };
    log::debug!(
        target: "model",
        id = model_id,
        dir = dir.display().to_string(),
        files = std::fs::read_dir(&dir).map(|e| e.count()).unwrap_or(0),
        found = found
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        "ev=model.search"
    );
    found
}

fn is_temp_name(name: &str) -> bool {
    name.ends_with(".part") || name.ends_with(".lock")
}

fn name_matches(path: &Path, needle: &str) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().to_lowercase().contains(needle))
}

fn scan_dir(dir: &Path, needle: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            let temp = path
                .file_name()
                .is_some_and(|n| is_temp_name(&n.to_string_lossy()));
            path.is_file() && !temp && name_matches(path, needle)
        })
}

pub(super) struct ModelSet {
    pub(super) vad: Option<SileroVad>,
    pub(super) gigaam: Option<GigaAM>,
    pub(super) ctc: Option<GigaCTC>,
    pub(super) whisper: Option<Whisper>,
    pub(super) embed: Option<EmbeddingExtractor>,
    pub(super) segmentation: Option<SegmentationModel>,
    pub(super) diar_loaded: bool,
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

fn source_channel(channel: Option<i32>, channels: usize) -> Result<usize, String> {
    match channel {
        None if channels == 1 => Ok(0),
        Some(channel) if channel >= 0 && (channel as usize) < channels => Ok(channel as usize),
        _ => Err(
            "Источник голоса неоднозначен — повторите расшифровку и выберите чистый участок"
                .to_string(),
        ),
    }
}

pub(crate) fn extract_utterance_embedding(
    audio: &Path,
    start: f64,
    end: f64,
    channel: Option<i32>,
) -> Result<(Vec<f64>, f64, f64), String> {
    // WAV читается только запрошенным фрагментом (2–6 с): проверка по всему
    // файлу запрещала бы обучение голоса из длинной записи без причины.
    let is_wav = audio
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"));
    if !is_wav {
        memory::check(audio)?;
    }
    if end - start < 0.4 {
        return Err("Фрагмент короче 0.4 с: голос не извлечь".to_string());
    }
    if !mr_audio::has_audio_track(audio) {
        return Err("Файл не содержит аудиодорожки".to_string());
    }
    if let Some((_, channels)) = mr_audio::decode::media_info(audio) {
        source_channel(channel, channels)?;
    }
    let snippet = is_wav
        .then(|| {
            // Отрицательный канал не клампим в 0: общий путь ниже вернёт понятную ошибку.
            let ch = match channel {
                Some(c) => Some(usize::try_from(c).ok()?),
                None => None,
            };
            mr_audio::decode::decode_wav_range_16k(audio, start, end, ch).ok()
        })
        .flatten();
    let samples: Vec<f32> = match snippet {
        Some(samples) => samples,
        None => {
            let channels = decode_to_channels_16k_auto(audio).map_err(|e| e.to_string())?;
            let ch = source_channel(channel, channels.len())?;
            let samples = channels
                .get(ch)
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
    models.ensure_diar();
    // Тот же AGC-фронтенд, что и при диаризации/верификации: иначе эталон
    // на сыром тихом канале расходится с центроидами.
    let raw_rms =
        (samples.iter().map(|&x| (x * x) as f64).sum::<f64>() / samples.len().max(1) as f64).sqrt();
    let mut clean_samples = samples;
    let (agc_plan, agc_stats) = mr_audio::dsp::agc_analysis_16k(&clean_samples);
    if agc_stats.amplified_fraction > 0.0 {
        mr_audio::dsp::agc_apply_16k(&mut clean_samples, &agc_plan);
    }
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
    let quality =
        ((raw_rms / 0.05).clamp(0.3, 1.2) * (clean_dur / (end - start).max(0.4)).min(1.0) * 1000.0)
            .round()
            / 1000.0;
    Ok((emb, clean_dur, quality))
}

impl ModelSet {
    fn load(plan: LoadPlan) -> Result<Self, String> {
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
        Ok(Self {
            vad: None,
            gigaam,
            ctc,
            whisper,
            embed: None,
            segmentation: None,
            diar_loaded: false,
        })
    }

    pub(super) fn ensure_vad(&mut self) -> Result<&mut SileroVad, String> {
        if self.vad.is_none() {
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
            self.vad = Some(vad);
        }
        Ok(self.vad.as_mut().expect("только что загрузили"))
    }

    pub(super) fn ensure_diar(&mut self) {
        if self.diar_loaded {
            return;
        }
        self.diar_loaded = true;
        let emb_path = find_model_file("diar-eres2netv2-emb", ".onnx")
            .or_else(|| find_model_file("diar-pyannote-emb", ".onnx"))
            .or_else(|| find_model_file("diar-wespeaker-resnet34", ".onnx"));
        self.embed = emb_path.and_then(|path| match EmbeddingExtractor::load(&path) {
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
        self.segmentation =
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
    }
}

#[cfg(test)]
mod source_tests {
    use super::source_channel;

    #[test]
    fn enrollment_requires_explicit_source_for_stereo() {
        assert_eq!(source_channel(None, 1).expect("моно"), 0);
        assert_eq!(source_channel(Some(1), 2).expect("правый канал"), 1);
        assert!(source_channel(None, 2).is_err());
        assert!(source_channel(Some(2), 2).is_err());
        assert!(source_channel(Some(-1), 2).is_err());
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::find_model_file;
    use std::path::PathBuf;

    fn with_data_dir(tag: &str) -> (PathBuf, Option<std::ffi::OsString>) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("mr-model-resolve-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var_os("MEETING_RECORDER_DATA_DIR");
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);
        (dir, old)
    }

    fn restore(old: Option<std::ffi::OsString>, dir: &std::path::Path) {
        match old {
            Some(value) => std::env::set_var("MEETING_RECORDER_DATA_DIR", value),
            None => std::env::remove_var("MEETING_RECORDER_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolves_files_in_manifest_order_and_skips_temp_files() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (dir, old) = with_data_dir("order");
        let model = dir.join("models").join("asr-gigaam-v3-rnnt-int8");
        std::fs::create_dir_all(&model).unwrap();
        for name in [
            "gigaam_dec_q.onnx",
            "gigaam_enc_q.onnx",
            "gigaam_enc_q.onnx.part",
            "junk_enc.onnx",
        ] {
            std::fs::write(model.join(name), b"x").unwrap();
        }
        let found = find_model_file("asr-gigaam-v3-rnnt-int8", ".onnx").expect("файл модели");
        assert_eq!(
            found.file_name().unwrap().to_string_lossy(),
            "gigaam_enc_q.onnx",
            "первый файл манифеста, а не мусор в каталоге"
        );
        let decoder = find_model_file("asr-gigaam-v3-rnnt-int8", "dec").expect("декодер");
        assert_eq!(
            decoder.file_name().unwrap().to_string_lossy(),
            "gigaam_dec_q.onnx"
        );
        restore(old, &dir);
    }

    #[test]
    fn unknown_id_falls_back_to_directory_scan() {
        let _guard = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (dir, old) = with_data_dir("legacy");
        let model = dir.join("models").join("diar-wespeaker-resnet34");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(model.join("emb.onnx.part"), b"x").unwrap();
        std::fs::write(model.join("emb.onnx"), b"x").unwrap();
        let found = find_model_file("diar-wespeaker-resnet34", ".onnx").expect("старый каталог");
        assert_eq!(found.file_name().unwrap().to_string_lossy(), "emb.onnx");
        restore(old, &dir);
    }
}
