use std::collections::HashMap;

use super::session::OrtSession;

pub const WINDOW_SAMPLES: usize = 160_000;
pub const STEP_SAMPLES: usize = 32_000;
pub const FRAME_SAMPLES: usize = 270;
pub const POWERSET_CLASSES: usize = 7;
const RECEPTIVE_FIELD_SAMPLES: usize = 721;
const BATCH_SIZE: usize = 8;
const VAD_SPEECH_PROB: f32 = 0.15;
const MIN_SPEECH_VAD_FRAMES: usize = 3;
/// Сумма вероятностей парных классов powerset надёжнее argmax: она
/// инвариантна к перестановке локальных спикеров между окнами PIT.
/// shortcut: 0.35 без замера на размеченных перекрытиях; откалибровать при смене модели.
const OVERLAP_THRESHOLD: f32 = 0.35;

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
    /// Нули на месте перекрытия давали бы в fbank кадры цифровой тишины,
    /// которые искажают CMN и attentive pooling голосового эмбеддинга.
    pub fn exclude_overlap(&self, samples: &mut [f32], offset: usize) -> usize {
        let end = offset + samples.len();
        let mut write = 0usize;
        let mut read = 0usize;
        let first = offset / FRAME_SAMPLES;
        for (index, &class) in self.frames.iter().enumerate().skip(first) {
            let start = index * FRAME_SAMPLES;
            if start >= end {
                break;
            }
            if class < 4 {
                continue;
            }
            let lo = start.max(offset).saturating_sub(offset);
            let hi = (start + FRAME_SAMPLES).min(end).saturating_sub(offset);
            if lo >= hi {
                continue;
            }
            if lo > read {
                samples.copy_within(read..lo, write);
                write += lo - read;
            }
            read = read.max(hi);
        }
        if read < samples.len() {
            samples.copy_within(read.., write);
            write += samples.len() - read;
        }
        write
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
        self.segment_impl(samples, None)
    }

    /// VAD уже посчитан конвейером: окна без речи не гоняем через PyAnnote.
    pub fn segment_with_vad(
        &mut self,
        samples: &[f32],
        vad_probs: &[f32],
    ) -> Result<Segmentation, String> {
        self.segment_impl(samples, Some(vad_probs))
    }

    fn segment_impl(
        &mut self,
        samples: &[f32],
        vad_probs: Option<&[f32]>,
    ) -> Result<Segmentation, String> {
        if samples.is_empty() {
            return Ok(Segmentation {
                frames: vec![],
                windows: vec![],
            });
        }
        let mut sums: Vec<[f32; POWERSET_CLASSES]> = vec![];
        let mut overlaps: Vec<f32> = vec![];
        let mut counts: Vec<u32> = vec![];
        let mut windows = vec![];
        let dynamic_batch = self
            .session
            .input_shape(&self.input_name)
            .and_then(|shape| shape.first().copied())
            .is_some_and(|dim| dim < 0);
        let mut pending: Vec<(usize, usize)> = vec![];
        let mut start = 0usize;
        loop {
            let valid = (samples.len() - start).min(WINDOW_SAMPLES);
            let digital_silence = samples[start..start + valid].iter().all(|x| x.abs() < 1e-4);
            let speech = !digital_silence
                && vad_probs.is_none_or(|probs| window_has_speech(probs, start, valid));
            if !speech {
                let valid_frames = frames_for_samples(valid);
                extend_timeline(&mut sums, &mut overlaps, &mut counts, start, valid_frames);
                windows.push(SegmentationWindow {
                    start_sample: start,
                    classes: vec![0; valid_frames],
                });
            } else if dynamic_batch {
                pending.push((start, valid));
            } else {
                self.run_windows(
                    samples,
                    &[(start, valid)],
                    &mut sums,
                    &mut overlaps,
                    &mut counts,
                    &mut windows,
                )?;
            }
            if start + WINDOW_SAMPLES >= samples.len() {
                break;
            }
            start += STEP_SAMPLES;
        }
        for chunk in pending.chunks(BATCH_SIZE) {
            self.run_windows(
                samples,
                chunk,
                &mut sums,
                &mut overlaps,
                &mut counts,
                &mut windows,
            )?;
        }
        let frames = sums
            .iter()
            .zip(&overlaps)
            .zip(&counts)
            .map(|((scores, &overlap), &count)| frame_class(scores, overlap, count))
            .collect();
        Ok(Segmentation { frames, windows })
    }

    #[allow(clippy::too_many_arguments)]
    fn run_windows(
        &mut self,
        samples: &[f32],
        batch: &[(usize, usize)],
        sums: &mut Vec<[f32; POWERSET_CLASSES]>,
        overlaps: &mut Vec<f32>,
        counts: &mut Vec<u32>,
        windows: &mut Vec<SegmentationWindow>,
    ) -> Result<(), String> {
        let mut input = vec![0.0f32; batch.len() * WINDOW_SAMPLES];
        for (slot, &(start, valid)) in batch.iter().enumerate() {
            let offset = slot * WINDOW_SAMPLES;
            input[offset..offset + valid].copy_from_slice(&samples[start..start + valid]);
        }
        let mut inputs = HashMap::new();
        inputs.insert(
            self.input_name.clone(),
            (vec![batch.len() as i64, 1, WINDOW_SAMPLES as i64], input),
        );
        let outputs = match self.session.run_f32(inputs) {
            Ok(outputs) => outputs,
            Err(_) if batch.len() > 1 => {
                // Модель объявила динамический батч, но конкретный граф его
                // не принял. Падать в последовательный режим, а не терять
                // диаризацию целиком.
                for &(start, valid) in batch {
                    self.run_windows(samples, &[(start, valid)], sums, overlaps, counts, windows)?;
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let (shape, data) = outputs
            .get(&self.output_name)
            .ok_or_else(|| "segmentation-модель не вернула logits".to_string())?;
        let batch_len = batch.len() as i64;
        if shape.len() != 3
            || shape[0] != batch_len
            || shape[2] != POWERSET_CLASSES as i64
            || data.len() % (batch.len() * POWERSET_CLASSES) != 0
        {
            return Err(format!(
                "ожидался выход [{batch_len}, frames, 7], получен {shape:?}"
            ));
        }
        let output_frames = data.len() / (batch.len() * POWERSET_CLASSES);
        for (slot, &(start, valid)) in batch.iter().enumerate() {
            let offset = slot * output_frames * POWERSET_CLASSES;
            let rows = &data[offset..offset + output_frames * POWERSET_CLASSES];
            write_window(
                start,
                valid,
                output_frames,
                rows,
                sums,
                overlaps,
                counts,
                windows,
            );
        }
        Ok(())
    }
}

fn window_has_speech(vad: &[f32], start: usize, valid: usize) -> bool {
    let lo = start / super::vad::VAD_WINDOW;
    let hi = (start + valid)
        .div_ceil(super::vad::VAD_WINDOW)
        .min(vad.len());
    if lo >= hi {
        return false;
    }
    vad[lo..hi]
        .iter()
        .filter(|&&prob| prob >= VAD_SPEECH_PROB)
        .count()
        >= MIN_SPEECH_VAD_FRAMES
}

#[allow(clippy::too_many_arguments)]
fn write_window(
    start: usize,
    valid: usize,
    output_frames: usize,
    rows: &[f32],
    sums: &mut Vec<[f32; POWERSET_CLASSES]>,
    overlaps: &mut Vec<f32>,
    counts: &mut Vec<u32>,
    windows: &mut Vec<SegmentationWindow>,
) {
    let valid_frames = if valid == WINDOW_SAMPLES {
        output_frames
    } else {
        frames_for_samples(valid).min(output_frames)
    };
    extend_timeline(sums, overlaps, counts, start, valid_frames);
    let mut classes = Vec::with_capacity(valid_frames);
    let (logits, _) = rows.as_chunks::<POWERSET_CLASSES>();
    for (frame, logits) in logits.iter().take(valid_frames).enumerate() {
        let probabilities = softmax(logits);
        classes.push(argmax(&probabilities));
        let global = (start + frame * FRAME_SAMPLES + FRAME_SAMPLES / 2) / FRAME_SAMPLES;
        for class in 0..POWERSET_CLASSES {
            sums[global][class] += probabilities[class];
        }
        overlaps[global] += probabilities[4] + probabilities[5] + probabilities[6];
        counts[global] += 1;
    }
    windows.push(SegmentationWindow {
        start_sample: start,
        classes,
    });
}

fn frames_for_samples(samples: usize) -> usize {
    samples.saturating_sub(RECEPTIVE_FIELD_SAMPLES) / FRAME_SAMPLES
}

/// Пропущенное окно всё равно занимает отрезок таймлайна: без расширения
/// индексы `frames` следующих окон сдвинулись бы на длину паузы.
fn extend_timeline(
    sums: &mut Vec<[f32; POWERSET_CLASSES]>,
    overlaps: &mut Vec<f32>,
    counts: &mut Vec<u32>,
    start: usize,
    frames: usize,
) {
    let Some(last_frame) = frames.checked_sub(1) else {
        return;
    };
    let last = (start + last_frame * FRAME_SAMPLES + FRAME_SAMPLES / 2) / FRAME_SAMPLES;
    if last >= sums.len() {
        sums.resize(last + 1, [0.0; POWERSET_CLASSES]);
        overlaps.resize(last + 1, 0.0);
        counts.resize(last + 1, 0);
    }
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

fn frame_class(scores: &[f32; POWERSET_CLASSES], overlap: f32, count: u32) -> u8 {
    if count == 0 {
        return 0;
    }
    if overlap / count as f32 >= OVERLAP_THRESHOLD {
        return 4;
    }
    argmax(&scores[..4])
}

fn argmax(scores: &[f32]) -> u8 {
    let mut best = 0usize;
    for (index, &score) in scores.iter().enumerate().skip(1) {
        if score > scores[best] {
            best = index;
        }
    }
    best as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powerset_contract_and_softmax_decode() {
        assert_eq!(frames_for_samples(WINDOW_SAMPLES), 589);
        assert_eq!(argmax(&softmax(&[4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])), 0);
        assert_eq!(argmax(&softmax(&[0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 2.0])), 5);
        assert_eq!(argmax(&[1.0; POWERSET_CLASSES]), 0);
        let probabilities = softmax(&[0.0; POWERSET_CLASSES]);
        assert!((probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn vad_gate_skips_windows_without_speech() {
        let mut probs = vec![0.0f32; 1000];
        assert!(!window_has_speech(&probs, 0, WINDOW_SAMPLES));
        probs[10] = 0.9;
        probs[11] = 0.9;
        assert!(!window_has_speech(&probs, 0, WINDOW_SAMPLES));
        probs[12] = 0.9;
        assert!(window_has_speech(&probs, 0, WINDOW_SAMPLES));
        assert!(!window_has_speech(&probs, 900_000, WINDOW_SAMPLES));
    }

    #[test]
    fn overlap_is_removed_and_clean_audio_compacted() {
        let segmentation = Segmentation {
            frames: vec![1, 4, 0, 0],
            windows: vec![],
        };
        let original: Vec<f32> = (0..FRAME_SAMPLES * 4).map(|i| i as f32).collect();
        let mut samples = original.clone();
        let clean = segmentation.exclude_overlap(&mut samples, 0);
        assert_eq!(clean, FRAME_SAMPLES * 3);
        assert_eq!(&samples[..FRAME_SAMPLES], &original[..FRAME_SAMPLES]);
        assert_eq!(
            &samples[FRAME_SAMPLES..clean],
            &original[FRAME_SAMPLES * 2..]
        );
    }

    #[test]
    fn overlap_outside_window_keeps_offsets() {
        let segmentation = Segmentation {
            frames: vec![1, 4, 6, 0, 2],
            windows: vec![],
        };
        let mut samples = vec![1.0; FRAME_SAMPLES * 2];
        let clean = segmentation.exclude_overlap(&mut samples, FRAME_SAMPLES * 3);
        assert_eq!(clean, FRAME_SAMPLES * 2);
        assert!(samples.iter().all(|&sample| sample == 1.0));
    }

    #[test]
    fn overlap_probability_sum_ignores_speaker_permutation() {
        // Перестановка спикеров 1↔2 переставляет классы 1↔2 и 5↔6,
        // но сумма парных классов 4+5+6 не меняется.
        let scores = [0.05, 0.35, 0.1, 0.1, 0.25, 0.0, 0.15];
        let permuted = [0.05, 0.1, 0.35, 0.1, 0.25, 0.15, 0.0];
        assert_eq!(frame_class(&scores, 0.4, 1), 4);
        assert_eq!(frame_class(&permuted, 0.4, 1), 4);
        // Без оверлапа выбирается только одиночный класс, а не «хвост» оверлапа.
        assert_eq!(frame_class(&scores, 0.2, 1), 1);
        assert_eq!(frame_class(&[0.0; POWERSET_CLASSES], 0.0, 0), 0);
    }
}
