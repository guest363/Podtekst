use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "podtekst",
    version = env!("CARGO_PKG_VERSION"),
    about = "Подтекст: локальная запись и расшифровка встреч (Rust-ядро)"
)]
pub(crate) struct Cli {
    #[arg(long, default_value_t = DEFAULT_HTTP_PORT)]
    pub(crate) port: u16,
    #[arg(long)]
    pub(crate) web: bool,
    #[arg(long)]
    pub(crate) browser: bool,
    #[arg(long)]
    pub(crate) window: bool,
    #[arg(long)]
    pub(crate) no_browser: bool,
    #[arg(long)]
    pub(crate) tray: bool,
    #[arg(long)]
    pub(crate) toggle: bool,
    #[arg(long, alias = "record")]
    pub(crate) start: bool,
    #[arg(long)]
    pub(crate) stop: bool,
    #[arg(long)]
    pub(crate) show: bool,
    #[arg(long)]
    pub(crate) hide: bool,
    #[arg(long)]
    pub(crate) quit: bool,
    #[arg(long)]
    pub(crate) autostart_on: bool,
    #[arg(long)]
    pub(crate) autostart_off: bool,
    #[arg(long, value_name = "FILE")]
    pub(crate) transcribe: Option<PathBuf>,
    #[arg(long, default_value = "auto")]
    pub(crate) engine: String,
    #[arg(long, default_value = "auto")]
    pub(crate) language: String,
    #[arg(long, default_value = "Я")]
    pub(crate) speaker_l: String,
    #[arg(long, default_value = "Собеседник")]
    pub(crate) speaker_r: String,
    #[arg(long, default_value_t = 0, help = "Минимум спикеров (0 — авто)")]
    pub(crate) min_speakers: usize,
    #[arg(long, default_value_t = 0, help = "Максимум спикеров (0 — авто)")]
    pub(crate) max_speakers: usize,
    #[arg(
        long,
        value_enum,
        default_value = "auto",
        help = "CTC-уточнение GigaAM: auto — как в настройках (по умолчанию вкл), off — без уточнения, on — принудительно"
    )]
    pub(crate) ctc: CtcMode,
    #[arg(long, hide = true)]
    pub(crate) macos_selftest: bool,
    #[arg(long, help = "Собрать отчёт диагностики и выйти")]
    pub(crate) diagnostics: bool,
    #[arg(long, help = "Добавить в отчёт хвост dev-логов (только dev-сборка)")]
    pub(crate) with_dev_logs: bool,
}

impl Cli {
    pub(crate) fn validate_speakers(&self) -> Result<(), String> {
        if self.min_speakers > 0 && self.max_speakers > 0 && self.min_speakers > self.max_speakers {
            return Err(format!(
                "--min-speakers ({}) не может быть больше --max-speakers ({})",
                self.min_speakers, self.max_speakers
            ));
        }
        Ok(())
    }

    /// Опции конвейера собираются в одном месте: иначе новый флаг легко потерять
    /// между разбором командной строки и расшифровкой.
    pub(crate) fn transcribe_options(&self) -> crate::transcribe::TranscribeOptions {
        crate::transcribe::TranscribeOptions {
            speaker_left: self.speaker_l.clone(),
            speaker_right: self.speaker_r.clone(),
            engine: self.engine.clone(),
            language: self.language.to_lowercase(),
            min_speakers: self.min_speakers,
            max_speakers: self.max_speakers,
            hotwords: crate::config::configured_hotwords(),
            ctc_enabled: self.ctc.allows(crate::config::configured_ctc_enabled()),
        }
    }
}

/// A/B CTC-уточнения GigaAM: `auto` уважает переключатель из «Настроек» (по умолчанию вкл),
/// `off` принудительно выключает, `on` принудительно включает в CLI-прогоне.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum CtcMode {
    Auto,
    Off,
    On,
}

impl CtcMode {
    pub(crate) fn allows(self, configured: bool) -> bool {
        match self {
            Self::Auto => configured,
            Self::Off => false,
            Self::On => true,
        }
    }
}

pub(crate) const DEFAULT_HTTP_PORT: u16 = 8765;
