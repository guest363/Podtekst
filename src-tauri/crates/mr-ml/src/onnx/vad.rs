use std::collections::HashMap;

use super::session::OrtSession;

pub const VAD_WINDOW: usize = 512;
pub const VAD_CONTEXT: usize = 64;
const STATE_DIMS: [usize; 3] = [2, 1, 128];
// ~0.5 с тишины прогоняем через модель, чтобы LSTM и контекст перешли
// в состояние тишины; дальше inference пропускаем до полного сброса.
const SILENT_INFERENCE_WINDOWS: usize = 16;
const SILENT_RESET_WINDOWS: usize = 32;

#[derive(Debug, PartialEq)]
enum SilenceAction {
    Process,
    Skip,
    Reset,
}

fn silence_action(silent_run: usize) -> SilenceAction {
    if silent_run <= SILENT_INFERENCE_WINDOWS {
        SilenceAction::Process
    } else if silent_run == SILENT_RESET_WINDOWS {
        SilenceAction::Reset
    } else {
        SilenceAction::Skip
    }
}

/// Счётчики решения VAD: без них не видно, почему модель «не увидела» речь —
/// цифровая тишина, сброс состояния LSTM или реальный низкий порог.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VadRunStats {
    pub windows: usize,
    pub silent_skip: usize,
    pub silent_infer: usize,
    pub resets: usize,
    pub non_finite: usize,
}

pub struct SileroVad {
    session: OrtSession,
    state: Vec<f32>,
    context: Vec<f32>,
    sample_rate: i64,
}

impl SileroVad {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            session: OrtSession::load(path, 1)?,
            state: vec![0.0; STATE_DIMS.iter().product()],
            context: vec![0.0; VAD_CONTEXT],
            sample_rate: 16000,
        })
    }

    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
    }

    pub fn process_window(&mut self, window: &[f32]) -> Result<f32, String> {
        let mut full_input = Vec::with_capacity(VAD_CONTEXT + VAD_WINDOW);
        full_input.extend_from_slice(&self.context);
        let n = window.len().min(VAD_WINDOW);
        full_input.extend_from_slice(&window[..n]);
        if full_input.len() < VAD_CONTEXT + VAD_WINDOW {
            full_input.resize(VAD_CONTEXT + VAD_WINDOW, 0.0);
        }

        // Контекст — последние 64 сэмпла реально поданного потока, без хвоста,
        // добитого нулями до окна модели: иначе короткое последнее окно
        // обнуляло бы контекст и следующий вызов терял конец фразы.
        let real_len = VAD_CONTEXT + n;
        self.context
            .copy_from_slice(&full_input[real_len - VAD_CONTEXT..real_len]);

        let mut f32_in = HashMap::new();
        f32_in.insert(
            "input".to_string(),
            (vec![1i64, (VAD_CONTEXT + VAD_WINDOW) as i64], full_input),
        );
        f32_in.insert(
            "state".to_string(),
            (
                STATE_DIMS.iter().map(|&x| x as i64).collect(),
                self.state.clone(),
            ),
        );
        let mut i64_in = HashMap::new();
        i64_in.insert(
            "sr".to_string(),
            (Vec::<i64>::new(), vec![self.sample_rate]),
        );

        let out = self.session.run_mixed(f32_in, i64_in)?;
        let prob = out
            .get("output")
            .and_then(|(_, d)| d.first().copied())
            .unwrap_or(0.0);
        if let Some((_, s)) = out.get("stateN") {
            if s.len() == self.state.len() {
                self.state.copy_from_slice(s);
            }
        }
        Ok(prob)
    }

    pub fn process_all(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        self.process_all_with(samples, |_, _| {})
    }

    /// То же с прогрессом и пропуском цифровой тишины. На Android один
    /// inference на окно 32 мс — главный тормоз часовых записей; окна полной
    /// тишины не отправляем в ONNX, а сразу считаем неречевыми.
    pub fn process_all_with<F>(&mut self, samples: &[f32], progress: F) -> Result<Vec<f32>, String>
    where
        F: FnMut(usize, usize),
    {
        self.process_all_with_stats(samples, progress)
            .map(|(probs, _)| probs)
    }

    pub fn process_all_with_stats<F>(
        &mut self,
        samples: &[f32],
        mut progress: F,
    ) -> Result<(Vec<f32>, VadRunStats), String>
    where
        F: FnMut(usize, usize),
    {
        self.reset();
        let chunks: Vec<&[f32]> = samples.chunks(VAD_WINDOW).collect();
        let total = chunks.len();
        let mut probs = Vec::with_capacity(total);
        let mut silent_run = 0usize;
        let mut stats = VadRunStats {
            windows: total,
            ..VadRunStats::default()
        };
        for (i, chunk) in chunks.into_iter().enumerate() {
            let peak = chunk.iter().fold(0.0f32, |acc, x| acc.max(x.abs()));
            if peak < 1e-4 {
                silent_run += 1;
                match silence_action(silent_run) {
                    SilenceAction::Process => {
                        // Вероятность известной цифровой тишины не нужна,
                        // но прогон обновляет state и context модели.
                        self.process_window(chunk)?;
                        stats.silent_infer += 1;
                    }
                    SilenceAction::Reset => {
                        self.reset();
                        stats.resets += 1;
                    }
                    SilenceAction::Skip => stats.silent_skip += 1,
                }
                probs.push(0.0);
            } else {
                silent_run = 0;
                let probability = self.process_window(chunk)?;
                if !probability.is_finite() {
                    stats.non_finite += 1;
                }
                probs.push(probability);
            }
            if i % 256 == 0 || i + 1 == total {
                progress(i + 1, total);
            }
        }
        Ok((probs, stats))
    }
}

/// К началу добавляются два окна pre-roll, чтобы сохранить начальные согласные.
pub fn vad_segments_from_probs(
    probs: &[f32],
    threshold: f32,
    min_speech_sec: f32,
    min_silence_sec: f32,
) -> Vec<(f32, f32)> {
    const PREROLL_WINDOWS: usize = 2;
    let win_sec = VAD_WINDOW as f32 / 16000.0;
    let mut segs = Vec::new();
    let mut start: Option<usize> = None;
    let mut silence_run = 0usize;
    let mut last_end = 0usize;
    let min_sil_windows = (min_silence_sec / win_sec).round() as usize;
    for (i, &p) in probs.iter().enumerate() {
        if p >= threshold {
            if start.is_none() {
                start = Some(i);
            }
            silence_run = 0;
        } else if let Some(s) = start {
            silence_run += 1;
            if silence_run >= min_sil_windows.max(1) {
                let a = s.saturating_sub(PREROLL_WINDOWS).max(last_end);
                let b = i - silence_run + 1;
                if (b as f32 - a as f32) * win_sec >= min_speech_sec {
                    segs.push((a as f32 * win_sec, b as f32 * win_sec));
                }
                last_end = b;
                start = None;
                silence_run = 0;
            }
        }
    }
    if let Some(s) = start {
        let a = s.saturating_sub(PREROLL_WINDOWS).max(last_end);
        let b = probs.len();
        if (b as f32 - a as f32) * win_sec >= min_speech_sec {
            segs.push((a as f32 * win_sec, b as f32 * win_sec));
        }
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_updates_state_before_skipping() {
        assert_eq!(silence_action(1), SilenceAction::Process);
        assert_eq!(
            silence_action(SILENT_INFERENCE_WINDOWS),
            SilenceAction::Process
        );
        assert_eq!(
            silence_action(SILENT_INFERENCE_WINDOWS + 1),
            SilenceAction::Skip
        );
        assert_eq!(silence_action(SILENT_RESET_WINDOWS), SilenceAction::Reset);
        assert_eq!(
            silence_action(SILENT_RESET_WINDOWS + 1),
            SilenceAction::Skip
        );
    }

    #[test]
    fn segments_hysteresis() {
        let mut probs = vec![0.9f32; 31];
        probs.extend(vec![0.05; 3]);
        probs.extend(vec![0.9; 31]);
        let segs = vad_segments_from_probs(&probs, 0.5, 0.25, 0.3);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].1 - segs[0].0 > 1.8);
    }

    #[test]
    fn segments_include_two_preroll_windows() {
        let mut probs = vec![0.0f32; 5];
        probs.extend(vec![0.9; 10]);
        probs.extend(vec![0.0; 16]);
        let segs = vad_segments_from_probs(&probs, 0.5, 0.25, 0.5);
        assert_eq!(segs.len(), 1);
        assert!((segs[0].0 - 3.0 * 0.032).abs() < 1e-6);
        assert!((segs[0].1 - 15.0 * 0.032).abs() < 1e-6);
    }
}
