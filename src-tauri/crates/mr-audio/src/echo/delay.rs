use super::fft::{self, Cx};

const FRAME: usize = 1 << 14;
#[cfg(test)]
pub(crate) const MIN_WINDOW_SAMPLES: usize = FRAME;

pub fn estimate(mic: &[f32], reference: &[f32], max_delay: usize) -> Option<(usize, f32)> {
    let n = mic.len().min(reference.len());
    if n < FRAME {
        return None;
    }
    let spectrum_len = FRAME;
    let mut acc = vec![Cx::default(); spectrum_len];
    let mut csum = vec![Cx::default(); spectrum_len / 2];
    let mut den_d = vec![0.0f32; spectrum_len / 2];
    let mut den_x = vec![0.0f32; spectrum_len / 2];
    let mut frames = 0usize;
    let mut bins = 0usize;
    let mut d_buf = vec![Cx::default(); spectrum_len];
    let mut x_buf = vec![Cx::default(); spectrum_len];
    let low = (400.0 * FRAME as f32 / 16000.0) as usize;
    let high = (spectrum_len * 78 / 100).min(spectrum_len / 2);
    let mut d_mag = vec![0.0f32; high];
    for start in (0..=(n - FRAME)).step_by(FRAME) {
        for (i, b) in d_buf.iter_mut().enumerate() {
            *b = Cx::new(mic[start + i], 0.0);
        }
        for (i, b) in x_buf.iter_mut().enumerate() {
            *b = Cx::new(reference[start + i], 0.0);
        }
        fft::fft(&mut d_buf);
        fft::fft(&mut x_buf);
        if frame_power(&d_buf) < 1e-8 || frame_power(&x_buf) < 1e-8 {
            continue;
        }
        let mut max_mag = 0.0f32;
        for k in 1..spectrum_len / 2 {
            max_mag = max_mag.max(d_buf[k].norm2().sqrt() * x_buf[k].norm2().sqrt());
        }
        let gate = max_mag * 0.01;
        for k in low..high {
            let dm = d_buf[k].norm2();
            let cross = d_buf[k].mul_conj(x_buf[k]);
            d_buf[k] = cross;
            d_mag[k] = dm;
        }
        for k in low..high {
            let cross = d_buf[k];
            let m = cross.norm2().sqrt();
            if m < gate {
                continue;
            }
            bins += 1;
            let norm = Cx::new(cross.re / m, cross.im / m);
            acc[k] = Cx::new(acc[k].re + norm.re, acc[k].im + norm.im);
            acc[spectrum_len - k] = Cx::new(
                acc[spectrum_len - k].re + norm.re,
                acc[spectrum_len - k].im - norm.im,
            );
            csum[k] = Cx::new(csum[k].re + cross.re, csum[k].im + cross.im);
            den_d[k] += d_mag[k];
            den_x[k] += x_buf[k].norm2();
        }
        if bins > 0 {
            frames += 1;
        }
    }
    if frames == 0 || bins < frames * (high - low) / 20 {
        return None;
    }
    for k in low..high {
        let coh = csum[k].norm2() / (den_d[k] * den_x[k] + 1e-12);
        let w = coh.min(1.0);
        acc[k] = acc[k].scale(w);
        acc[spectrum_len - k] = acc[spectrum_len - k].scale(w);
    }
    fft::ifft(&mut acc);
    let search_end = max_delay.min(spectrum_len / 2);
    let vals: Vec<f32> = acc[..search_end]
        .iter()
        .map(|x| x.re / frames as f32)
        .collect();
    let (best_idx, &peak) = vals
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap_or((0, &0.0));
    if peak <= 0.0 {
        return None;
    }
    let mean_abs = vals.iter().map(|v| v.abs()).sum::<f32>() / vals.len() as f32;
    Some((best_idx, peak / mean_abs.max(1e-9)))
}

fn frame_power(spectrum: &[Cx]) -> f32 {
    spectrum.iter().map(|x| x.norm2()).sum::<f32>() / spectrum.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chirp(len: usize, f0: f32, f1: f32) -> Vec<f32> {
        let mut phase = 0.0f32;
        (0..len)
            .map(|i| {
                let f = f0 + (f1 - f0) * (i as f32 / len as f32);
                phase += std::f32::consts::TAU * f / 16000.0;
                phase.sin()
            })
            .collect()
    }

    #[test]
    fn finds_shifted_copy_of_reference() {
        let reference = chirp(FRAME * 3, 200.0, 4000.0);
        let near = chirp(FRAME * 3, 4100.0, 7000.0);
        let delay = 2400usize;
        let mut mic = vec![0.0f32; delay];
        mic.extend(reference[..reference.len() - delay].iter().copied());
        for (i, m) in mic.iter_mut().enumerate() {
            *m += 0.3 * near[i];
        }
        let (delay_found, prom) =
            estimate(&mic, &reference, 8000).expect("эхо должно быть найдено");
        assert!(
            (delay_found as i64 - delay as i64).abs() <= 2,
            "задержка {delay_found} вместо {delay}"
        );
        println!("echo prom={prom}");
        assert!(prom > 10.0, "проминентность {prom}");
    }

    #[test]
    fn uncorrelated_channels_have_no_confident_peak() {
        let reference = chirp(FRAME * 2, 200.0, 4000.0);
        let mut seed = 0x1234567890ABCDEFu64;
        let mut mic = Vec::with_capacity(FRAME * 2);
        for _ in 0..FRAME * 2 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            mic.push((seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0);
        }
        if let Some((_, prom)) = estimate(&mic, &reference, 8000) {
            println!("noecho prom={prom}");
            assert!(prom < 6.5, "проминентность {prom}");
        }
    }

    #[test]
    fn narrowband_hum_is_rejected() {
        let reference = chirp(FRAME * 2, 220.0, 225.0);
        let mut mic = vec![0.0f32; 1600];
        mic.extend(reference[..reference.len() - 1600].iter().copied());
        if let Some((_, prom)) = estimate(&mic, &reference, 8000) {
            println!("narrowband prom={prom}");
            assert!(prom < 6.5, "проминентность {prom}");
        }
    }
}
