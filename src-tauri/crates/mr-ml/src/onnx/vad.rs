//! Стриминговый Silero VAD v6 (v6.2.3, ONNX): окна 512 сэмплов @16 кГц,
//! контекст 64 сэмпла, состояние [2,1,128] переносится между чанками.

use std::collections::HashMap;

use super::session::OrtSession;

pub const VAD_WINDOW: usize = 512;
pub const VAD_CONTEXT: usize = 64;
const STATE_DIMS: [usize; 3] = [2, 1, 128];

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

    /// Вероятность речи в окне ровно 512 сэмплов с 64 сэмплами контекста (Silero VAD v6).
    pub fn process_window(&mut self, window: &[f32]) -> Result<f32, String> {
        let mut full_input = Vec::with_capacity(VAD_CONTEXT + VAD_WINDOW);
        full_input.extend_from_slice(&self.context);
        let n = window.len().min(VAD_WINDOW);
        full_input.extend_from_slice(&window[..n]);
        if full_input.len() < VAD_CONTEXT + VAD_WINDOW {
            full_input.resize(VAD_CONTEXT + VAD_WINDOW, 0.0);
        }

        self.context
            .copy_from_slice(&full_input[full_input.len() - VAD_CONTEXT..]);

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

    /// Прогон всего сигнала окнами 512, возвращает вероятность на окно.
    pub fn process_all(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        self.reset();
        let mut probs = Vec::new();
        for chunk in samples.chunks(VAD_WINDOW) {
            probs.push(self.process_window(chunk)?);
        }
        Ok(probs)
    }
}

/// Превращает вероятности окон в сегменты речи [start,end] в секундах.
pub fn vad_segments_from_probs(
    probs: &[f32],
    threshold: f32,
    min_speech_sec: f32,
    min_silence_sec: f32,
) -> Vec<(f32, f32)> {
    let win_sec = VAD_WINDOW as f32 / 16000.0;
    let mut segs = Vec::new();
    let mut start: Option<usize> = None;
    let mut silence_run = 0usize;
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
                let (a, b) = (s, i - silence_run + 1);
                if (b as f32 - a as f32) * win_sec >= min_speech_sec {
                    segs.push((a as f32 * win_sec, b as f32 * win_sec));
                }
                start = None;
                silence_run = 0;
            }
        }
    }
    if let Some(s) = start {
        let b = probs.len();
        if (b as f32 - s as f32) * win_sec >= min_speech_sec {
            segs.push((s as f32 * win_sec, b as f32 * win_sec));
        }
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_hysteresis() {
        // 1 с "речи" (p=0.9), 0.1 с паузы, снова речь — при min_silence 0.3 с склеивается
        let mut probs = vec![0.9f32; 31];
        probs.extend(vec![0.05; 3]);
        probs.extend(vec![0.9; 31]);
        let segs = vad_segments_from_probs(&probs, 0.5, 0.25, 0.3);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].1 - segs[0].0 > 1.8);
    }
}
