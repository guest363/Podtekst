//! Резервный путь захвата на Linux: утилиты PipeWire/PulseAudio (`pw-record`,
//! `parec`).
//!
//! Основной путь — прямая работа через `libpulse` (см. `pulse.rs`).
//! Этот модуль нужен на системах, где системной библиотеки нет (минимальные
//! дистрибутивы, нестандартные сборки): запись всё равно работает, но
//! полагается на внешний процесс.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;

use super::stream::{s16le_to_f32, LevelMeter, RawWriter};
use super::{AudioError, CaptureStream, Result};

/// Первый доступный рекордер в `PATH`.
pub fn recorder_binary() -> Option<&'static str> {
    ["pw-record", "parec"]
        .into_iter()
        .find(|bin| Command::new(bin).arg("--version").output().is_ok())
}

/// Запустить утилиту записи для источника `device` в сырой файл s16le.
fn spawn_recorder(device: &str, raw: &Path, rate: u32) -> Result<Child> {
    if let Some(parent) = raw.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bin = recorder_binary().ok_or_else(|| {
        AudioError::NoDevice(
            "не найден pw-record/parec и нет системной библиотеки PulseAudio".to_string(),
        )
    })?;
    let mut cmd = Command::new(bin);
    if bin == "pw-record" {
        cmd.args([
            "--target",
            device,
            "--rate",
            &rate.to_string(),
            "--channels",
            "1",
            "--format",
            "s16",
            "--raw",
        ])
        .arg(raw);
    } else {
        // parec пишет raw s16le в stdout → перенаправляем в файл.
        let f = std::fs::File::create(raw)?;
        cmd.args([
            "--device",
            device,
            "--format=s16le",
            &format!("--rate={rate}"),
            "--channels=1",
        ])
        .stdout(f);
    }
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    cmd.spawn()
        .map_err(|e| AudioError::Record(format!("не стартовал захват {device}: {e}")))
}

/// Захват через внешнюю утилиту: сырой s16le дописывается в файл,
/// параллельно идёт хвостовое чтение и перевод в общий формат f32.
pub fn start(device: &str, path: &Path, rate: u32) -> Result<CaptureStream> {
    let raw = path.with_extension("s16");
    let _ = std::fs::remove_file(&raw);
    let mut child = spawn_recorder(device, &raw, rate)?;

    let raw_for_thread = raw.clone();
    let path = path.to_path_buf();
    CaptureStream::spawn(path, move |path, level, stop| {
        let mut writer = match RawWriter::create(path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Не удалось создать файл записи: {e}");
                return;
            }
        };
        // Дескриптор держим открытым: переоткрытие стоило 10 системных вызовов/с.
        let mut tail: Option<(std::fs::File, u64)> = None;
        while !stop.load(Ordering::Relaxed) {
            read_new(&raw_for_thread, &mut tail, &mut writer, level);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // Финальный дочит и остановка процесса-записи.
        let _ = child.kill();
        let _ = child.wait();
        read_new(&raw_for_thread, &mut tail, &mut writer, level);
        let _ = writer.flush();
        let _ = std::fs::remove_file(&raw_for_thread);
    })
}

/// Дочитать новые байты из файла утилиты и записать их в общий формат.
fn read_new(
    raw: &Path,
    tail: &mut Option<(std::fs::File, u64)>,
    writer: &mut RawWriter,
    level: &std::sync::Arc<LevelMeter>,
) {
    use std::io::{Read, Seek, SeekFrom};
    if tail.is_none() {
        let Ok(file) = std::fs::File::open(raw) else {
            return;
        };
        *tail = Some((file, 0));
    }
    let Some((file, offset)) = tail.as_mut() else {
        return;
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return;
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return;
    }
    if buf.is_empty() {
        return;
    }
    *offset += buf.len() as u64;
    let samples = s16le_to_f32(&buf);
    if samples.is_empty() {
        return;
    }
    level.update(&samples);
    if let Err(e) = writer.write_samples(&samples) {
        eprintln!("Ошибка диска при записи звука: {e}");
    }
}

/// Список источников через `pactl` (только для резервного пути).
pub fn pactl_sources() -> Vec<String> {
    let Ok(out) = Command::new("pactl")
        .env("LC_ALL", "C")
        .args(["list", "short", "sources"])
        .output()
    else {
        return vec![];
    };
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|s| s.to_string())
        .collect()
}

/// Монитор выхода по умолчанию через `pactl` (только для резервного пути).
pub fn pactl_default_monitor() -> Option<String> {
    let out = Command::new("pactl")
        .env("LC_ALL", "C")
        .arg("get-default-sink")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sink = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sink.is_empty() {
        return None;
    }
    let mon = format!("{sink}.monitor");
    pactl_sources().into_iter().find(|s| s == &mon)
}
