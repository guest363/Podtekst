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

pub fn recorder_binary() -> Option<&'static str> {
    ["pw-record", "parec"]
        .into_iter()
        .find(|bin| Command::new(bin).arg("--version").output().is_ok())
}

fn spawn_recorder(device: &str, raw: &Path, rate: u32, log: &Path) -> Result<Child> {
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
    // stderr в файл, а не в null: причина смерти рекордера нужна в ошибке,
    // но пайп без чтения заблокировал бы процесс на заполнении буфера.
    let log_file = std::fs::File::create(log)?;
    cmd.stdin(Stdio::null()).stderr(Stdio::from(log_file));
    cmd.spawn()
        .map_err(|e| AudioError::Record(format!("не стартовал захват {device}: {e}")))
}

/// Последние байты stderr внешнего рекордера для сообщения об ошибке.
fn recorder_tail(log: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(log) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file.seek(SeekFrom::Start(len.saturating_sub(400))).is_err() {
        return String::new();
    }
    let mut tail = String::new();
    let _ = file.take(400).read_to_string(&mut tail);
    tail.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Умер ли рекордер сам (без нашей команды остановки).
fn died_early(recorder: &mut RecorderGuard) -> Option<std::process::ExitStatus> {
    let child = recorder.0.as_mut()?;
    child.try_wait().ok().flatten()
}

pub fn start(device: &str, path: &Path, rate: u32) -> Result<CaptureStream> {
    let raw = path.with_extension("s16");
    let log = path.with_extension("s16.stderr");
    let _ = std::fs::remove_file(&raw);
    let mut recorder = RecorderGuard(Some(spawn_recorder(device, &raw, rate, &log)?));

    // Рекордер может умереть сразу (нет устройства, занято, нет прав). Без этой
    // проверки старт «успешен», а дорожка остаётся пустой без единой ошибки.
    for _ in 0..15 {
        if let Some(status) = died_early(&mut recorder) {
            let reason = recorder_tail(&log);
            let _ = std::fs::remove_file(&raw);
            let _ = std::fs::remove_file(&log);
            return Err(AudioError::Record(format!(
                "захват {device} завершился сразу ({status}): {reason}"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let raw_for_thread = raw.clone();
    let log_for_thread = log.clone();
    let path = path.to_path_buf();
    let stream = CaptureStream::spawn(path, move |path, level, stop| {
        let mut writer = match RawWriter::create(path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Не удалось создать файл записи: {e}");
                recorder.kill_and_wait();
                let _ = std::fs::remove_file(&raw_for_thread);
                return;
            }
        };
        // Дескриптор держим открытым: переоткрытие — это 10 системных вызовов в секунду.
        let mut tail: Option<(std::fs::File, u64)> = None;
        while !stop.load(Ordering::Relaxed) {
            if let Some(status) = died_early(&mut recorder) {
                let reason = recorder_tail(&log_for_thread);
                log::error!(
                    target: "audio",
                    status = status.to_string(),
                    reason = reason;
                    "ev=audio.capture.exited"
                );
                eprintln!("Захват звука прекратился ({status}): {reason}");
                break;
            }
            read_new(&raw_for_thread, &mut tail, &mut writer, level);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        recorder.kill_and_wait();
        read_new(&raw_for_thread, &mut tail, &mut writer, level);
        let _ = writer.flush();
        let _ = std::fs::remove_file(&raw_for_thread);
        let _ = std::fs::remove_file(&log_for_thread);
    });
    if stream.is_err() {
        let _ = std::fs::remove_file(&raw);
    }
    stream
}

/// Внешний рекордер живёт, пока жив guard: без этого при ошибке создания
/// файла или потока `pw-record`/`parec` продолжал бы писать в сырой файл.
struct RecorderGuard(Option<Child>);

impl RecorderGuard {
    fn kill_and_wait(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for RecorderGuard {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

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
    // Непарный байт — половина сэмпла от ещё дописываемого файла: без сдвига
    // на чётную границу весь остаток дорожки читается мусором.
    let usable = buf.len() - buf.len() % 2;
    *offset += usable as u64;
    let samples = s16le_to_f32(&buf[..usable]);
    if samples.is_empty() {
        return;
    }
    level.update(&samples);
    if let Err(e) = writer.write_samples(&samples) {
        eprintln!("Ошибка диска при записи звука: {e}");
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mr-cli-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn odd_trailing_byte_does_not_shift_the_stream() {
        let dir = tmp_dir("odd-byte");
        let raw = dir.join("mic.s16");
        let out = dir.join("mic.f32");
        // Один полный сэмпл 0x1000 и половина следующего.
        std::fs::write(&raw, [0x00u8, 0x10, 0x20]).unwrap();

        let mut writer = RawWriter::create(&out).unwrap();
        let level = LevelMeter::new();
        let mut tail = None;
        read_new(&raw, &mut tail, &mut writer, &level);
        writer.flush().unwrap();

        // Дописываем вторую половину: сэмпл должен собраться как 0x3020.
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(&raw).unwrap();
        file.write_all(&[0x30]).unwrap();
        drop(file);
        read_new(&raw, &mut tail, &mut writer, &level);
        writer.flush().unwrap();

        let samples = crate::capture::read_raw_f32(&out).unwrap();
        assert_eq!(samples.len(), 2, "{samples:?}");
        assert!((samples[0] - 4096.0 / 32768.0).abs() < 1e-6, "{samples:?}");
        assert!((samples[1] - 12320.0 / 32768.0).abs() < 1e-6, "{samples:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
