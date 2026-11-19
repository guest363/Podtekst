//! Замер скорости конвейера для локального A/B (`MR_PERF=1`).
//!
//! Схема sidecar `<stem>.perf.json` аддитивна к `.transcript.json` и не
//! участвует в контракте расшифровки. Калибровка нужна, чтобы сравнивать
//! машины с разной загрузкой: сырой RTF остаётся только для одной машины.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use serde::Serialize;

const SCHEMA: u32 = 1;
const CALIB_VERSION: u32 = 1;
const CALIB_REPS: usize = 5;
// В debug-тестах кернел маленький, чтобы не тратить секунды; перф-числа
// сравниваются только между release-сборками на одной машине.
const CALIB_DIM: usize = if cfg!(debug_assertions) { 96 } else { 448 };

static ENABLED: OnceLock<bool> = OnceLock::new();
static CALIB: OnceLock<Calibration> = OnceLock::new();

struct State {
    depth: usize,
    sheet: Option<Sheet>,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State { depth: 0, sheet: None }) };
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
struct Calibration {
    version: u32,
    ms: f64,
    dim: usize,
    profile: &'static str,
    logical_cpus: usize,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
struct Stages {
    decode: f64,
    models: f64,
    vad_model: f64,
    vad: f64,
    asr: f64,
    diar: f64,
    diar_models: f64,
    json: f64,
}

impl Stages {
    fn add(&mut self, name: &str, ms: f64) {
        match name {
            "decode" => self.decode += ms,
            "models" => self.models += ms,
            "vad_model" => self.vad_model += ms,
            "vad" => self.vad += ms,
            "asr" => self.asr += ms,
            "diar" => self.diar += ms,
            "diar_models" => self.diar_models += ms,
            "json" => self.json += ms,
            _ => {}
        }
    }

    /// `diar_models` вложена в `diar`, поэтому в сумму верхнего уровня не входит.
    fn upper_sum(self) -> f64 {
        self.decode + self.models + self.vad_model + self.vad + self.asr + self.diar + self.json
    }
}

#[derive(Debug, Default, Clone, Serialize)]
struct Counters {
    utterances: usize,
    words: usize,
    speakers: usize,
    speech_sec: f64,
}

#[derive(Debug, Default)]
struct Sheet {
    stages: Stages,
    counters: Counters,
    audio_sec: f64,
    input_channels: usize,
    processed_channels: usize,
    engine: String,
    ctc: bool,
    calib: Calibration,
}

#[derive(Debug, Serialize)]
struct PerfDoc {
    schema: u32,
    commit: &'static str,
    engine: String,
    ctc: bool,
    audio_sec: f64,
    input_channels: usize,
    processed_channels: usize,
    total_ms: f64,
    stages_ms: Stages,
    other_ms: f64,
    peak_rss_kb: Option<u64>,
    calib: Calibration,
    counters: Counters,
}

fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var("MR_PERF")
            .map(|value| {
                let value = value.trim();
                value == "1" || value.eq_ignore_ascii_case("true")
            })
            .unwrap_or(false)
    })
}

/// Замер одного файла; `Drop` владельца очищает лист при раннем выходе.
#[derive(Debug)]
pub(super) struct Run {
    started: Instant,
    owner: bool,
    armed: bool,
}

impl Run {
    /// Забирает лист только у владельца и ровно один раз уменьшает depth.
    fn release(&mut self) -> Option<Sheet> {
        if !self.armed {
            return None;
        }
        self.armed = false;
        let owner = self.owner;
        STATE
            .try_with(|cell| {
                let mut state = cell.try_borrow_mut().ok()?;
                let sheet = if owner { state.sheet.take() } else { None };
                state.depth = state.depth.saturating_sub(1);
                sheet
            })
            .ok()
            .flatten()
    }

    /// Пишет sidecar рядом с `.transcript.json`; ошибка записи не ломает расшифровку.
    pub(super) fn finish(mut self, json_path: &Path) {
        let total_ms = self.started.elapsed().as_secs_f64() * 1000.0;
        let Some(sheet) = self.release() else {
            return;
        };
        let peak_rss_kb =
            crate::sysmem::peak_mb(std::process::id()).map(|mb| (mb * 1024.0).round() as u64);
        let doc = PerfDoc {
            schema: SCHEMA,
            commit: option_env!("PODTEKST_GIT_COMMIT").unwrap_or("unknown"),
            engine: sheet.engine,
            ctc: sheet.ctc,
            audio_sec: sheet.audio_sec,
            input_channels: sheet.input_channels,
            processed_channels: sheet.processed_channels,
            total_ms,
            stages_ms: sheet.stages,
            other_ms: other_ms(total_ms, &sheet.stages),
            peak_rss_kb,
            calib: sheet.calib,
            counters: sheet.counters,
        };
        let Some(path) = sidecar_path(json_path) else {
            return;
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
            if let Err(error) = crate::fsutil::write_atomic(&path, &bytes) {
                eprintln!("perf sidecar: {error}");
            }
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        // Паника в Drop недопустима: при уничтожении TLS состояние уже недоступно.
        let _ = self.release();
    }
}

/// Начинает замер; вложенный begin не перезаписывает внешний лист.
pub(super) fn begin() -> Option<Run> {
    if !enabled() {
        return None;
    }
    let calib = *CALIB.get_or_init(calibrate);
    let owner = STATE
        .try_with(|cell| {
            let mut state = cell.try_borrow_mut().ok()?;
            state.depth += 1;
            if state.depth == 1 {
                state.sheet = Some(Sheet {
                    calib,
                    ..Sheet::default()
                });
                Some(true)
            } else {
                Some(false)
            }
        })
        .ok()
        .flatten()?;
    Some(Run {
        started: Instant::now(),
        owner,
        armed: true,
    })
}

fn update_sheet(update: impl FnOnce(&mut Sheet)) {
    if !enabled() {
        return;
    }
    let _ = STATE.try_with(|cell| {
        if let Ok(mut state) = cell.try_borrow_mut() {
            if state.depth == 1 {
                if let Some(sheet) = state.sheet.as_mut() {
                    update(sheet);
                }
            }
        }
    });
}

pub(super) struct Stage {
    name: &'static str,
    started: Instant,
}

impl Drop for Stage {
    fn drop(&mut self) {
        // Паника в Drop недопустима: при уничтожении TLS лист может быть уже недоступен.
        let ms = self.started.elapsed().as_secs_f64() * 1000.0;
        let _ = STATE.try_with(|cell| {
            if let Ok(mut state) = cell.try_borrow_mut() {
                if state.depth == 1 {
                    if let Some(sheet) = state.sheet.as_mut() {
                        sheet.stages.add(self.name, ms);
                    }
                }
            }
        });
    }
}

/// RAII-таймер стадии: только верхний уровень пишет в лист.
pub(super) fn stage(name: &'static str) -> Option<Stage> {
    if !enabled() {
        return None;
    }
    let top = STATE
        .try_with(|cell| {
            cell.try_borrow()
                .map(|state| state.depth == 1)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    top.then(|| Stage {
        name,
        started: Instant::now(),
    })
}

pub(super) fn set_audio(audio_sec: f64, input_channels: usize) {
    update_sheet(|sheet| {
        sheet.audio_sec = audio_sec;
        sheet.input_channels = input_channels;
    });
}

pub(super) fn set_processed_channels(processed_channels: usize) {
    update_sheet(|sheet| sheet.processed_channels = processed_channels);
}

/// Счётчики фиксируются до записи sidecar: текст и имена сюда не попадают (P12).
pub(super) fn set_results(
    utterances: usize,
    words: usize,
    speakers: usize,
    speech_sec: f64,
    engine: &str,
    ctc: bool,
) {
    update_sheet(|sheet| {
        sheet.counters = Counters {
            utterances,
            words,
            speakers,
            speech_sec,
        };
        sheet.engine = engine.to_string();
        sheet.ctc = ctc;
    });
}

fn other_ms(total_ms: f64, stages: &Stages) -> f64 {
    (total_ms - stages.upper_sum()).max(0.0)
}

/// `X.transcript.json` → `X.perf.json`; прочие имена не трогаем.
pub(super) fn sidecar_path(json_path: &Path) -> Option<PathBuf> {
    let name = json_path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".transcript.json")?;
    Some(json_path.with_file_name(format!("{stem}.perf.json")))
}

fn calibrate() -> Calibration {
    let mut best = f64::INFINITY;
    for _ in 0..CALIB_REPS {
        let started = Instant::now();
        let checksum = cpu_kernel(CALIB_DIM);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(checksum);
        best = best.min(elapsed);
    }
    // Один поток: параллельный кернел мерил бы планировщик, а не машину.
    Calibration {
        version: CALIB_VERSION,
        ms: best,
        dim: CALIB_DIM,
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        logical_cpus: std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1),
    }
}

/// Детерминированный f32-кернел: умножение матриц и цикл ln/exp.
fn cpu_kernel(dim: usize) -> f32 {
    let mut a = vec![0.0f32; dim * dim];
    let mut b = vec![0.0f32; dim * dim];
    for index in 0..dim * dim {
        a[index] = ((index % 97) as f32 - 48.0) / 48.0;
        b[index] = ((index % 89) as f32 - 44.0) / 44.0;
    }
    let a = std::hint::black_box(a);
    let b = std::hint::black_box(b);
    let mut c = vec![0.0f32; dim * dim];
    for i in 0..dim {
        for k in 0..dim {
            let aik = a[i * dim + k];
            for j in 0..dim {
                c[i * dim + j] += aik * b[k * dim + j];
            }
        }
    }
    let mut acc = 0.0f32;
    for (index, &value) in c.iter().enumerate() {
        let x = value.abs() * 1e-6 + (index % 7) as f32 + 1.0;
        acc += x.ln().exp() * 1e-9;
    }
    std::hint::black_box(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_is_positive_and_not_wildly_unstable() {
        let first = calibrate();
        let second = calibrate();
        assert!(first.ms > 0.0 && first.ms.is_finite());
        assert!(second.ms > 0.0 && second.ms.is_finite());
        let ratio = first.ms / second.ms;
        assert!(
            (0.05..=20.0).contains(&ratio),
            "калибровка нестабильна: {first:?} vs {second:?}"
        );
        assert!(first.logical_cpus > 0);
    }

    #[test]
    fn stage_times_accumulate_and_nested_stage_is_not_in_upper_sum() {
        let mut stages = Stages::default();
        stages.add("asr", 10.0);
        stages.add("asr", 5.5);
        stages.add("diar", 40.0);
        stages.add("diar_models", 3.0);
        assert!((stages.asr - 15.5).abs() < 1e-9);
        assert!((stages.diar_models - 3.0).abs() < 1e-9);
        assert!((stages.upper_sum() - 55.5).abs() < 1e-9);
    }

    #[test]
    fn other_time_never_goes_negative() {
        let mut stages = Stages::default();
        stages.add("decode", 30.0);
        assert!((other_ms(100.0, &stages) - 70.0).abs() < 1e-9);
        assert_eq!(other_ms(10.0, &stages), 0.0);
    }

    use std::sync::Once;
    use std::time::Duration;

    static ENABLE: Once = Once::new();

    fn enable() {
        ENABLE.call_once(|| std::env::set_var("MR_PERF", "1"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mr-perf-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sheet_parts() -> Option<(f64, usize, String)> {
        STATE.with(|cell| {
            cell.borrow().sheet.as_ref().map(|sheet| {
                (
                    sheet.stages.asr,
                    sheet.counters.utterances,
                    sheet.engine.clone(),
                )
            })
        })
    }

    #[test]
    fn dropped_run_clears_sheet_for_next_owner() {
        enable();
        let first = begin().expect("MR_PERF включён");
        assert!(first.owner);
        drop(stage("asr").expect("верхняя стадия"));
        drop(first);
        let second = begin().expect("MR_PERF включён");
        assert!(second.owner);
        assert_eq!(sheet_parts().map(|parts| parts.0), Some(0.0));
        drop(second);
    }

    #[test]
    fn nested_run_does_not_touch_outer_sheet_or_write_sidecar() {
        enable();
        let dir = temp_dir("nested");
        let outer = begin().expect("MR_PERF включён");
        assert!(outer.owner);
        set_audio(1.0, 1);
        set_results(0, 0, 0, 0.0, "outer", false);
        let inner = begin().expect("MR_PERF включён");
        assert!(!inner.owner);
        assert!(stage("asr").is_none());
        set_results(1, 2, 3, 4.0, "inner", true);
        inner.finish(&dir.join("inner.transcript.json"));
        assert!(!dir.join("inner.perf.json").exists());
        let parts = sheet_parts().expect("внешний лист жив");
        assert_eq!(parts.1, 0);
        assert_eq!(parts.2, "outer");
        drop(outer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn successful_finish_writes_sidecar_and_releases_depth() {
        enable();
        let dir = temp_dir("finish");
        let run = begin().expect("MR_PERF включён");
        let asr = stage("asr").expect("верхняя стадия");
        std::thread::sleep(Duration::from_millis(2));
        drop(asr);
        set_audio(2.0, 1);
        set_results(3, 4, 1, 1.5, "gigaam", true);
        let path = dir.join("x.transcript.json");
        run.finish(&path);
        let doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("x.perf.json")).unwrap()).unwrap();
        assert_eq!(doc["schema"], 1);
        assert_eq!(doc["engine"], "gigaam");
        assert_eq!(doc["counters"]["utterances"], 3);
        assert!(
            doc["stages_ms"]["asr"].as_f64().unwrap() > 0.0,
            "Stage::drop не записал стадию"
        );
        let next = begin().expect("MR_PERF включён");
        assert!(next.owner);
        drop(next);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn perf_doc_keeps_schema_and_stage_fields() {
        let doc = PerfDoc {
            schema: SCHEMA,
            commit: "test",
            engine: "gigaam".to_string(),
            ctc: true,
            audio_sec: 1.0,
            input_channels: 1,
            processed_channels: 1,
            total_ms: 10.0,
            stages_ms: Stages {
                asr: 4.0,
                ..Stages::default()
            },
            other_ms: 6.0,
            peak_rss_kb: None,
            calib: Calibration {
                version: 1,
                ms: 1.0,
                dim: 8,
                profile: "release",
                logical_cpus: 1,
            },
            counters: Counters::default(),
        };
        let value = serde_json::to_value(&doc).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["stages_ms"]["asr"], 4.0);
        assert_eq!(value["calib"]["dim"], 8);
        assert_eq!(value["calib"]["profile"], "release");
    }

    #[test]
    fn sidecar_path_replaces_transcript_suffix_only() {
        assert_eq!(
            sidecar_path(Path::new("/tmp/meeting.transcript.json")),
            Some(PathBuf::from("/tmp/meeting.perf.json"))
        );
        assert_eq!(sidecar_path(Path::new("/tmp/meeting.json")), None);
        assert_eq!(sidecar_path(Path::new("/tmp/meeting.perf.json")), None);
    }
}
