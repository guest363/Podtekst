use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use mr_audio::capture::{ActiveAudioClient, DetectionCapability};

use super::apps::{matched_clients, MatchedApp, GENERIC_CALL};
use crate::config::AutoDetectMode;

const START_DEBOUNCE_TICKS: u8 = 2;
const START_DEBOUNCE_MIN: Duration = Duration::from_secs(2);
const STOP_COOLDOWN: Duration = Duration::from_secs(15);
const SUPPRESS_RELEASE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptInfo {
    pub(crate) id: String,
    pub(crate) app: String,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TickContext {
    pub(super) is_recording: bool,
    pub(super) is_auto_recording: bool,
    pub(super) lifecycle_in_flight: bool,
    pub(super) self_pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DetectorAction {
    NotifyCall {
        slug: &'static str,
        label: &'static str,
    },
    StartAutoRecording {
        slug: &'static str,
    },
    StopAutoRecording {
        slug: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Idle,
    Debouncing {
        app: MatchedApp,
        since: Instant,
        ticks: u8,
    },
    Prompted {
        id: String,
        app: MatchedApp,
        last_seen: Instant,
    },
    AutoRecording {
        app: MatchedApp,
        last_seen: Instant,
    },
}

pub(super) struct Detector {
    phase: Phase,
    suppressed: HashMap<&'static str, Option<Instant>>,
    suppress_active_on_next_tick: bool,
    prompt_seq: u64,
}

impl Detector {
    pub(super) fn new() -> Self {
        Self {
            phase: Phase::Idle,
            suppressed: HashMap::new(),
            suppress_active_on_next_tick: false,
            prompt_seq: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.suppressed.clear();
        self.suppress_active_on_next_tick = false;
    }

    pub(super) fn prompt(&self) -> Option<PromptInfo> {
        match &self.phase {
            Phase::Prompted { id, app, .. } => Some(PromptInfo {
                id: id.clone(),
                app: app.label.to_string(),
            }),
            _ => None,
        }
    }

    pub(super) fn on_manual_start(&mut self) {
        self.suppress_active_on_next_tick = false;
        if matches!(
            self.phase,
            Phase::Debouncing { .. } | Phase::Prompted { .. }
        ) {
            self.phase = Phase::Idle;
        }
    }

    pub(super) fn on_manual_stop(&mut self) {
        if let Phase::AutoRecording { app, .. } = self.phase {
            self.suppressed.insert(app.slug, None);
            self.phase = Phase::Idle;
        }
        self.suppress_active_on_next_tick = true;
    }

    pub(super) fn on_dismiss(&mut self) {
        if let Phase::Prompted { app, .. } = self.phase {
            self.suppressed.insert(app.slug, None);
            self.phase = Phase::Idle;
        }
    }

    pub(super) fn on_auto_start_failed(&mut self) {
        if let Phase::AutoRecording { app, .. } = self.phase {
            self.suppressed.insert(app.slug, None);
            self.phase = Phase::Idle;
        }
    }

    pub(super) fn tick(
        &mut self,
        now: Instant,
        mode: AutoDetectMode,
        capability: DetectionCapability,
        clients: &[ActiveAudioClient],
        ctx: TickContext,
    ) -> Vec<DetectorAction> {
        if mode == AutoDetectMode::Off || capability == DetectionCapability::Unsupported {
            self.reset();
            return Vec::new();
        }
        if ctx.lifecycle_in_flight {
            return Vec::new();
        }

        // На macOS < 14.2 собственный захват Подтекста держит DeviceIsRunningSomewhere = 1,
        // поэтому автостарт и автостоп невозможны — ограничиваемся уведомлением.
        let effective =
            if capability == DetectionCapability::DeviceOnly && mode == AutoDetectMode::Auto {
                AutoDetectMode::Notify
            } else {
                mode
            };

        let matched = matched_clients(clients, capability, ctx.self_pid, ctx.is_recording);
        let keep_alive_slugs: HashSet<&'static str> = matched
            .iter()
            .filter(|m| m.keep_alive)
            .map(|m| m.app.slug)
            .collect();

        self.suppressed.retain(|slug, absent_since| {
            if keep_alive_slugs.contains(slug) {
                *absent_since = None;
                true
            } else {
                let since = *absent_since.get_or_insert(now);
                now.saturating_duration_since(since) < SUPPRESS_RELEASE
            }
        });

        if self.suppress_active_on_next_tick && !ctx.is_recording {
            for slug in &keep_alive_slugs {
                self.suppressed.insert(*slug, None);
            }
            self.suppress_active_on_next_tick = false;
        }

        if ctx.is_recording && !ctx.is_auto_recording {
            self.phase = Phase::Idle;
            return Vec::new();
        }

        if ctx.is_auto_recording {
            let (app, last_seen) = match self.phase {
                Phase::AutoRecording { app, last_seen } => (app, last_seen),
                _ => {
                    let app = matched
                        .iter()
                        .find(|m| m.can_start)
                        .or_else(|| matched.iter().find(|m| m.keep_alive))
                        .map(|m| m.app)
                        .unwrap_or(GENERIC_CALL);
                    self.phase = Phase::AutoRecording {
                        app,
                        last_seen: now,
                    };
                    return Vec::new();
                }
            };
            let still_alive =
                keep_alive_slugs.contains(app.slug) || matched.iter().any(|m| m.can_start);
            if still_alive {
                self.phase = Phase::AutoRecording {
                    app,
                    last_seen: now,
                };
                return Vec::new();
            }
            if now.saturating_duration_since(last_seen) >= STOP_COOLDOWN {
                self.phase = Phase::Idle;
                return vec![DetectorAction::StopAutoRecording { slug: app.slug }];
            }
            return Vec::new();
        }

        if matches!(self.phase, Phase::AutoRecording { .. }) {
            self.phase = Phase::Idle;
        }

        let candidate = matched
            .iter()
            .filter(|m| m.can_start && !self.suppressed.contains_key(m.app.slug))
            .map(|m| m.app)
            .next();

        match self.phase.clone() {
            Phase::Idle => {
                if let Some(app) = candidate {
                    self.phase = Phase::Debouncing {
                        app,
                        since: now,
                        ticks: 1,
                    };
                }
                Vec::new()
            }
            Phase::Debouncing { app, since, ticks } => match candidate {
                Some(cur) if cur.slug == app.slug => {
                    let next_ticks = ticks.saturating_add(1);
                    if next_ticks >= START_DEBOUNCE_TICKS
                        && now.saturating_duration_since(since) >= START_DEBOUNCE_MIN
                    {
                        if effective == AutoDetectMode::Auto {
                            self.phase = Phase::AutoRecording {
                                app,
                                last_seen: now,
                            };
                            vec![DetectorAction::StartAutoRecording { slug: app.slug }]
                        } else {
                            self.prompt_seq += 1;
                            self.phase = Phase::Prompted {
                                id: self.prompt_seq.to_string(),
                                app,
                                last_seen: now,
                            };
                            vec![DetectorAction::NotifyCall {
                                slug: app.slug,
                                label: app.label,
                            }]
                        }
                    } else {
                        self.phase = Phase::Debouncing {
                            app,
                            since,
                            ticks: next_ticks,
                        };
                        Vec::new()
                    }
                }
                Some(other) => {
                    self.phase = Phase::Debouncing {
                        app: other,
                        since: now,
                        ticks: 1,
                    };
                    Vec::new()
                }
                None => {
                    self.phase = Phase::Idle;
                    Vec::new()
                }
            },
            Phase::Prompted { id, app, last_seen } => {
                if effective == AutoDetectMode::Auto {
                    if let Some(cur) = candidate {
                        self.phase = Phase::AutoRecording {
                            app: cur,
                            last_seen: now,
                        };
                        return vec![DetectorAction::StartAutoRecording { slug: cur.slug }];
                    }
                    self.phase = Phase::Idle;
                    return Vec::new();
                }
                if keep_alive_slugs.contains(app.slug) {
                    self.phase = Phase::Prompted {
                        id,
                        app,
                        last_seen: now,
                    };
                } else if now.saturating_duration_since(last_seen) >= SUPPRESS_RELEASE {
                    self.phase = Phase::Idle;
                }
                Vec::new()
            }
            Phase::AutoRecording { .. } => Vec::new(),
        }
    }
}
