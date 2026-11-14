pub mod ctc;
pub mod embed;
pub mod features;
pub mod rnnt;
pub mod segmentation;
pub mod session;
pub mod vad;
pub mod whisper;

pub use session::OrtSession;

fn is_digital_silence(samples: &[f32]) -> bool {
    samples.iter().all(|&sample| sample == 0.0)
}

#[cfg(test)]
mod tests {
    use super::is_digital_silence;

    #[test]
    fn digital_silence_requires_exact_zero_samples() {
        assert!(is_digital_silence(&[0.0, -0.0]));
        assert!(!is_digital_silence(&[0.0, 1e-5]));
        assert!(!is_digital_silence(&[-1e-5, 0.0]));
        assert!(!is_digital_silence(&[f32::MIN_POSITIVE]));
    }
}
