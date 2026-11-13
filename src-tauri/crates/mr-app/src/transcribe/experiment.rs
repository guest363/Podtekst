use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Anchors {
    #[default]
    Turns,
    Windows,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Centroids {
    #[default]
    Frozen,
    Online,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Embedding {
    #[default]
    Frontend,
    Raw,
    UnitNorm,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Guards {
    #[default]
    On,
    Off,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub(super) struct Config {
    pub benchmark: bool,
    pub experiment: bool,
    pub anchors: Anchors,
    pub centroids: Centroids,
    pub embedding: Embedding,
    pub guards: Guards,
}

impl Config {
    fn parse(benchmark: Option<&str>, experiment: Option<&str>) -> Result<Self, String> {
        let mut config = Self {
            benchmark: benchmark == Some("1"),
            experiment: experiment.is_some(),
            ..Self::default()
        };
        let Some(experiment) = experiment else {
            return Ok(config);
        };
        if !config.benchmark {
            return Err("MR_DIAR_EXPERIMENT требует MR_DIAR_BENCH=1".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for option in experiment.split(',') {
            let Some((key, value)) = option.trim().split_once('=') else {
                return Err("MR_DIAR_EXPERIMENT: ожидается key=value через запятую".to_string());
            };
            let (key, value) = (key.trim(), value.trim());
            if !seen.insert(key) {
                return Err(format!("MR_DIAR_EXPERIMENT: повторный ключ {key}"));
            }
            match (key, value) {
                ("anchors", "turns") => config.anchors = Anchors::Turns,
                ("anchors", "windows") => config.anchors = Anchors::Windows,
                ("centroids", "frozen") => config.centroids = Centroids::Frozen,
                ("centroids", "online") => config.centroids = Centroids::Online,
                ("embedding", "frontend") => config.embedding = Embedding::Frontend,
                ("embedding", "raw") => config.embedding = Embedding::Raw,
                ("embedding", "unitnorm") => config.embedding = Embedding::UnitNorm,
                ("guards", "on") => config.guards = Guards::On,
                ("guards", "off") => config.guards = Guards::Off,
                _ => {
                    return Err(format!(
                        "MR_DIAR_EXPERIMENT: неизвестная опция {key}={value}"
                    ))
                }
            }
        }
        Ok(config)
    }

    fn from_environment() -> Result<Self, String> {
        let read = |name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name}: требуется UTF-8")),
        };
        let benchmark = read("MR_DIAR_BENCH")?;
        let experiment = read("MR_DIAR_EXPERIMENT")?;
        Self::parse(benchmark.as_deref(), experiment.as_deref())
    }

    pub(super) fn record(self) {
        let modes = serde_json::json!({
            "benchmark": self.benchmark,
            "experiment": self.experiment,
            "anchors": self.anchors,
            "centroids": self.centroids,
            "embedding": self.embedding,
            "guards": self.guards,
        });
        let mode = |key: &str| modes[key].as_str().unwrap_or_default();
        log::info!(
            target: "diar",
            benchmark = self.benchmark,
            experiment = self.experiment,
            anchors = mode("anchors"),
            centroids = mode("centroids"),
            embedding = mode("embedding"),
            guards = mode("guards");
            "ev=diar.experiment"
        );
        crate::logging::write_artifact_json("diar/experiment.json", &modes);
    }
}

static CONFIG: OnceLock<Result<Config, String>> = OnceLock::new();

pub(super) fn initialize() -> Result<&'static Config, String> {
    CONFIG
        .get_or_init(Config::from_environment)
        .as_ref()
        .map_err(Clone::clone)
}

pub(super) fn current() -> &'static Config {
    initialize().expect("эксперимент проверен до загрузки моделей")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_options_keep_n_defaults() {
        assert_eq!(Config::parse(None, None).unwrap(), Config::default());
        let baseline = Config::parse(Some("1"), None).unwrap();
        for option in [
            "anchors=turns",
            "centroids=frozen",
            "embedding=frontend",
            "guards=on",
        ] {
            let config = Config::parse(Some("1"), Some(option)).unwrap();
            assert_eq!(
                config,
                Config {
                    experiment: true,
                    ..baseline
                },
                "{option}"
            );
        }
    }

    #[test]
    fn all_options_are_independent_and_order_does_not_matter() {
        let defaults = Config {
            benchmark: true,
            experiment: true,
            ..Config::default()
        };
        for (option, expected) in [
            (
                "anchors=windows",
                Config {
                    anchors: Anchors::Windows,
                    ..defaults
                },
            ),
            (
                "centroids=online",
                Config {
                    centroids: Centroids::Online,
                    ..defaults
                },
            ),
            (
                "embedding=raw",
                Config {
                    embedding: Embedding::Raw,
                    ..defaults
                },
            ),
            (
                "embedding=unitnorm",
                Config {
                    embedding: Embedding::UnitNorm,
                    ..defaults
                },
            ),
            (
                "guards=off",
                Config {
                    guards: Guards::Off,
                    ..defaults
                },
            ),
        ] {
            assert_eq!(
                Config::parse(Some("1"), Some(option)).unwrap(),
                expected,
                "{option}"
            );
        }
        let config = Config::parse(
            Some("1"),
            Some(" guards=off,embedding=raw,centroids=online,anchors=windows "),
        )
        .unwrap();
        assert_eq!(
            config,
            Config {
                benchmark: true,
                experiment: true,
                anchors: Anchors::Windows,
                centroids: Centroids::Online,
                embedding: Embedding::Raw,
                guards: Guards::Off,
            }
        );
    }

    #[test]
    fn invalid_experiments_never_fall_back() {
        for benchmark in [None, Some(""), Some("0"), Some("true")] {
            assert!(Config::parse(benchmark, Some("guards=off")).is_err());
        }
        for option in [
            "",
            "guards",
            "guards=off,",
            "guards=off,guards=on",
            "anchors=turns,anchors=turns",
            "unknown=on",
            "embedding=other",
            "centroids=other",
            "guards=other",
            "anchors=other",
            "guards=off=on",
        ] {
            assert!(Config::parse(Some("1"), Some(option)).is_err(), "{option}");
        }
    }
}
