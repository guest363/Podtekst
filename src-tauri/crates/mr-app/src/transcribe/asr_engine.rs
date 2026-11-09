use std::path::PathBuf;

use mr_packs as packs;

pub(super) const GIGAAM_DIR: &str = "asr-gigaam-v3-rnnt-int8";
pub(super) const CTC_DIR: &str = "asr-gigaam-v3-e2e-ctc-int8";
pub(super) const WHISPER_DIR: &str = "asr-whisper-large-v3-turbo-q";

pub(super) fn models_root() -> PathBuf {
    packs::default_app_data_dir().join("models")
}

/// ASR-движок, который реально будет работать: остальные веса не читаем.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AsrEngine {
    Gigaam,
    Whisper,
}

impl AsrEngine {
    pub(super) fn uses_gigaam(self) -> bool {
        self == Self::Gigaam
    }

    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Gigaam => "gigaam",
            Self::Whisper => "whisper",
        }
    }

    pub(super) fn model_id(self) -> &'static str {
        match self {
            Self::Gigaam => "gigaam-v3-rnnt-int8",
            Self::Whisper => "whisper-large-v3-turbo-q",
        }
    }

    pub(super) fn display_name(self) -> &'static str {
        match self {
            Self::Gigaam => "GigaAM-v3 (русский язык)",
            Self::Whisper => "Whisper large-v3-turbo (30+ языков)",
        }
    }

    pub(super) fn other(self) -> Self {
        match self {
            Self::Gigaam => Self::Whisper,
            Self::Whisper => Self::Gigaam,
        }
    }

    /// Файл-маркер установки: по нему выбор движка не требует загрузки весов.
    pub(super) fn installed(self) -> bool {
        let (dir, marker) = match self {
            Self::Gigaam => (GIGAAM_DIR, "gigaam_enc_q.onnx"),
            Self::Whisper => (WHISPER_DIR, "whisper_enc_q.onnx"),
        };
        models_root().join(dir).join(marker).is_file()
    }

    pub(super) fn missing_error(self) -> String {
        let name = match self {
            Self::Gigaam => "GigaAM",
            Self::Whisper => "Whisper",
        };
        format!("Модель {name} не скачана: «Настройки» → «Модели ИИ».")
    }
}

/// Что читать с диска: незадействованные модели не тратят время и RSS.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct LoadPlan {
    pub(super) asr: Option<AsrEngine>,
    pub(super) ctc: bool,
}

impl LoadPlan {
    pub(super) fn new(asr: Option<AsrEngine>, ctc_enabled: bool) -> Self {
        Self {
            asr,
            ctc: ctc_enabled && asr == Some(AsrEngine::Gigaam),
        }
    }

    /// Кэш годится, если содержит запрошенный движок и CTC уже запрашивался:
    /// повторять неудачную загрузку необязательной модели каждую расшифровку нельзя.
    pub(super) fn covers(self, requested: Self) -> bool {
        let engine_ok = requested.asr.is_none() || requested.asr == self.asr;
        engine_ok && (!requested.ctc || self.ctc)
    }
}

pub(super) fn pick_asr_engine(
    engine: &str,
    language: &str,
    has_gigaam: bool,
    has_whisper: bool,
) -> Result<AsrEngine, String> {
    if !has_gigaam && !has_whisper {
        return Err(
            "Нет модели распознавания речи. Скачайте GigaAM или Whisper: «Настройки» → «Модели ИИ»."
                .to_string(),
        );
    }
    match engine {
        "whisper" => has_whisper
            .then_some(AsrEngine::Whisper)
            .ok_or_else(|| AsrEngine::Whisper.missing_error()),
        "gigaam" => has_gigaam
            .then_some(AsrEngine::Gigaam)
            .ok_or_else(|| AsrEngine::Gigaam.missing_error()),
        "auto" => {
            let language = language.trim();
            let russian = language.is_empty()
                || language.eq_ignore_ascii_case("auto")
                || language.eq_ignore_ascii_case("ru");
            if russian {
                Ok(if has_gigaam {
                    AsrEngine::Gigaam
                } else {
                    AsrEngine::Whisper
                })
            } else {
                Ok(if has_whisper {
                    AsrEngine::Whisper
                } else {
                    AsrEngine::Gigaam
                })
            }
        }
        other => Err(format!(
            "Неизвестный движок распознавания «{other}»: допустимы auto, gigaam, whisper."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{pick_asr_engine, AsrEngine, LoadPlan};

    #[test]
    fn explicit_engine_requires_its_own_model() {
        assert_eq!(
            pick_asr_engine("whisper", "ru", true, true),
            Ok(AsrEngine::Whisper)
        );
        assert_eq!(
            pick_asr_engine("gigaam", "en", true, true),
            Ok(AsrEngine::Gigaam)
        );
        assert!(pick_asr_engine("whisper", "en", true, false)
            .unwrap_err()
            .contains("Whisper"));
        assert!(pick_asr_engine("gigaam", "ru", false, true)
            .unwrap_err()
            .contains("GigaAM"));
    }

    #[test]
    fn auto_prefers_whisper_for_non_russian_language() {
        for language in ["en", "de", "zh", "EN"] {
            assert_eq!(
                pick_asr_engine("auto", language, true, true),
                Ok(AsrEngine::Whisper),
                "language = {language}"
            );
        }
        assert_eq!(
            pick_asr_engine("auto", "en", true, false),
            Ok(AsrEngine::Gigaam)
        );
    }

    #[test]
    fn auto_prefers_gigaam_for_russian_or_unknown_language() {
        for language in ["ru", "RU", "auto", "", "  "] {
            assert_eq!(
                pick_asr_engine("auto", language, true, true),
                Ok(AsrEngine::Gigaam),
                "language = {language}"
            );
        }
        assert_eq!(
            pick_asr_engine("auto", "ru", false, true),
            Ok(AsrEngine::Whisper)
        );
    }

    #[test]
    fn no_models_or_unknown_engine_is_an_error() {
        for language in ["ru", "en", "auto"] {
            assert!(pick_asr_engine("auto", language, false, false).is_err());
            assert!(pick_asr_engine("whisper", language, false, false).is_err());
            assert!(pick_asr_engine("gigaam", language, false, false).is_err());
        }
        assert!(pick_asr_engine("ctc", "ru", true, true)
            .unwrap_err()
            .contains("Неизвестный движок"));
    }

    #[test]
    fn load_plan_reads_only_selected_engine() {
        let gigaam = LoadPlan::new(Some(AsrEngine::Gigaam), true);
        assert_eq!(gigaam.asr, Some(AsrEngine::Gigaam));
        assert!(gigaam.ctc);

        let gigaam_no_ctc = LoadPlan::new(Some(AsrEngine::Gigaam), false);
        assert_eq!(gigaam_no_ctc.asr, Some(AsrEngine::Gigaam));
        assert!(!gigaam_no_ctc.ctc);

        let whisper = LoadPlan::new(Some(AsrEngine::Whisper), true);
        assert_eq!(whisper.asr, Some(AsrEngine::Whisper));
        assert!(!whisper.ctc, "CTC нужен только GigaAM");

        let embeddings = LoadPlan::new(None, true);
        assert_eq!(embeddings.asr, None);
        assert!(!embeddings.ctc);
    }

    #[test]
    fn cached_set_is_reused_only_for_the_same_engine() {
        let gigaam = LoadPlan::new(Some(AsrEngine::Gigaam), true);
        let whisper = LoadPlan::new(Some(AsrEngine::Whisper), true);
        assert!(gigaam.covers(gigaam));
        assert!(!gigaam.covers(whisper));
        assert!(!whisper.covers(gigaam));
        assert!(!LoadPlan::new(None, false).covers(gigaam));
        // Извлечению эмбеддингов ASR не нужен: подходит любой загруженный набор.
        assert!(gigaam.covers(LoadPlan::new(None, false)));
    }

    #[test]
    fn cache_without_ctc_is_reloaded_only_once() {
        let without_ctc = LoadPlan::new(Some(AsrEngine::Gigaam), false);
        let with_ctc = LoadPlan::new(Some(AsrEngine::Gigaam), true);
        assert!(!without_ctc.covers(with_ctc));
        // Запрос CTC уже был: даже если модель не нашлась, кэш больше не дёргаем.
        assert!(with_ctc.covers(with_ctc));
        assert!(with_ctc.covers(without_ctc));
    }
}
