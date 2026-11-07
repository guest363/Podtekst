//! G2: runtime sidecar, модель, хранилище артефактов и guard-ы.
//!
//! G3 подключает job/HTTP/SSE поверх этих модулей; до этого публичные
//! функции держим без вызовов, чтобы не менять контракт дважды.
#![cfg_attr(target_os = "android", allow(dead_code))]
#![allow(dead_code)]

mod backend;
pub mod config;
mod id;
mod memory;
#[cfg(not(target_os = "android"))]
mod process;
#[cfg(not(target_os = "android"))]
mod runtime_locator;
#[cfg(not(target_os = "android"))]
mod sidecar;
mod store;

use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryJobError {
    Internal(String),
}

static INIT: OnceLock<()> = OnceLock::new();

pub fn init() {
    INIT.get_or_init(|| {
        #[cfg(not(target_os = "android"))]
        {
            process::cleanup_orphans();
            sidecar::arm_idle_monitor();
        }
    });
}

pub fn shutdown() {
    #[cfg(not(target_os = "android"))]
    sidecar::kill_current();
}

pub fn invalidate_model_cache() {}

pub fn is_active() -> bool {
    false
}

pub fn supported() -> bool {
    !cfg!(target_os = "android")
}

pub fn has_summary(parent: &std::path::Path, stem: &str) -> bool {
    store::has_summary(parent, stem)
}
