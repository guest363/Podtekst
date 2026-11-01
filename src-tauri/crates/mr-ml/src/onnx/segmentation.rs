use std::collections::HashMap;

use super::session::OrtSession;

pub const WINDOW_SAMPLES: usize = 160_000;
pub const STEP_SAMPLES: usize = 16_000;
pub const FRAME_SAMPLES: usize = 270;
pub const POWERSET_CLASSES: usize = 7;
const RECEPTIVE_FIELD_SAMPLES: usize = 721;

#[derive(Debug, Clone)]
pub struct SegmentationWindow {
    pub start_sample: usize,
    pub classes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Segmentation {
    pub frames: Vec<u8>,
    pub windows: Vec<SegmentationWindow>,
}

impl Segmentation {
    pub fn exclude_overlap(&self, samples: &mut [f32], offset: usize) -> usize {
        let end = offset + samples.len();
        let mut clean = samples.len();
        for (index, &class) in self.frames.iter().enumerate() {
            if class < 4 {
                continue;
            }
            let start = index * FRAME_SAMPLES;
            let lo = start.max(offset);
            let hi = (start + FRAME_SAMPLES).min(end);
            if lo < hi {
                samples[lo - offset..hi - offset].fill(0.0);
                clean -= hi - lo;
            }
        }
        clean
    }
}

pub struct SegmentationModel {
    session: OrtSession,
    input_name: String,
    output_name: String,
}

impl SegmentationModel {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let session = OrtSession::load(path, 0)?;
        let input_name = ["input_values", "input", "waveform"]
            .iter()
            .find(|name| session.has_input(name))
            .map(|name| name.to_string())
            .or_else(|| session.input_names.first().cloned())
            .ok_or_else(|| "у segmentation-модели нет входов".to_string())?;
        let output_name = session
            .output_names
            .iter()
            .find(|name| name.as_str() == "logits")
            .cloned()
            .or_else(|| session.output_names.first().cloned())
            .ok_or_else(|| "у segmentation-модели нет выходов".to_string())?;
        Ok(Self {
            session,
            input_name,
            output_name,
        })
    }

    pub fn segment(&mut self, samples: &[f32]) -> Result<Segmentation, String> {
        if samples.is_empty() {
            return Ok(Segmentation {
                frames: vec![],
                windows: vec![],
            });
        }
        let mut sums: Vec<[f32; POWERSET_CLASSES]> = vec![];
        let mut counts: Vec<u32> = vec![];
        let mut windows = vec![];
        let mut start = 0usize;
        loop {
            let valid = (samples.len() - start).min(WINDOW_SAMPLES);
            let mut input = vec![0.0f32; WINDOW_SAMPLES];
            input[..valid].copy_from_slice(&samples[start..start + valid]);
            let mut inputs = HashMap::new();
            inputs.insert(
                self.input_name.clone(),
                (vec![1, 1, WINDOW_SAMPLES as i64], input),
            );
            let outputs = self.session.run_f32(inputs)?;
            let (shape, data) = outputs
                .get(&self.output_name)
                .ok_or_else(|| "segmentation-модель не вернула logits".to_string())?;
            if shape.len() != 3
                || shape[0] != 1
                || shape[2] != POWERSET_CLASSES as i64
                || data.len() % POWERSET_CLASSES != 0
            {
                return Err(format!("ожидался выход [1, frames, 7], получен {shape:?}"));
            }
            let output_frames = data.len() / POWERSET_CLASSES;
            let valid_frames = if valid == WINDOW_SAMPLES {
                output_frames
            } else {
                frames_for_samples(valid).min(output_frames)
            };
            let mut classes = Vec::with_capacity(valid_frames);
            let (rows, _) = data.as_chunks::<POWERSET_CLASSES>();
            for (frame, logits) in rows.iter().take(valid_frames).enumerate() {
                let probabilities = softmax(logits);
                classes.push(argmax(&probabilities));
                let global = (start + frame * FRAME_SAMPLES + FRAME_SAMPLES / 2) / FRAME_SAMPLES;
                if global >= sums.len() {
                    sums.resize(global + 1, [0.0; POWERSET_CLASSES]);
                    counts.resize(global + 1, 0);
                }
                for class in 0..POWERSET_CLASSES {
                    sums[global][class] += probabilities[class];
                }
                counts[global] += 1;
            }
            windows.push(SegmentationWindow {
                start_sample: start,
                classes,
            });
            if start + WINDOW_SAMPLES >= samples.len() {
                break;
            }
            start += STEP_SAMPLES;
        }
        let frames = sums
            .iter()
            .zip(&counts)
            .map(|(scores, &count)| if count > 0 { argmax(scores) } else { 0 })
            .collect();
        Ok(Segmentation { frames, windows })
    }
}

fn frames_for_samples(samples: usize) -> usize {
    samples.saturating_sub(RECEPTIVE_FIELD_SAMPLES) / FRAME_SAMPLES
}

fn softmax(logits: &[f32]) -> [f32; POWERSET_CLASSES] {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut out = [0.0; POWERSET_CLASSES];
    let mut sum = 0.0;
    for (out, &logit) in out.iter_mut().zip(logits) {
        *out = (logit - max).exp();
        sum += *out;
    }
    if sum > 0.0 {
        for value in &mut out {
            *value /= sum;
        }
    }
    out
}

fn argmax(scores: &[f32; POWERSET_CLASSES]) -> u8 {
    scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(index, _)| index as u8)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powerset_contract_and_softmax_decode() {
        assert_eq!(frames_for_samples(WINDOW_SAMPLES), 589);
        assert_eq!(argmax(&softmax(&[4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])), 0);
        assert_eq!(argmax(&softmax(&[0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 2.0])), 5);
        let probabilities = softmax(&[0.0; POWERSET_CLASSES]);
        assert!((probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn overlap_is_zeroed_with_absolute_offsets() {
        let segmentation = Segmentation {
            frames: vec![1, 4, 6, 0, 2],
            windows: vec![],
        };
        let mut samples = vec![1.0; FRAME_SAMPLES * 2];
        let clean = segmentation.exclude_overlap(&mut samples, FRAME_SAMPLES);
        assert_eq!(clean, 0);
        assert!(samples.iter().all(|&sample| sample == 0.0));

        let mut samples = vec![1.0; FRAME_SAMPLES * 2];
        let clean = segmentation.exclude_overlap(&mut samples, FRAME_SAMPLES * 3);
        assert_eq!(clean, FRAME_SAMPLES * 2);
        assert!(samples.iter().all(|&sample| sample == 1.0));
    }
}
