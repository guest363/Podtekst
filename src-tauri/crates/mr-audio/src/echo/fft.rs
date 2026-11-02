//! Компактный radix-2 БПФ. Офлайн-обработке хватает f32, а внешние крейты
//! запрещены правилом P2 — поэтому собственная реализация.

#[derive(Clone, Copy, Default)]
pub(crate) struct Cx {
    pub re: f32,
    pub im: f32,
}

impl Cx {
    pub fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    pub fn mul(self, o: Cx) -> Cx {
        Cx::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }

    pub fn mul_conj(self, o: Cx) -> Cx {
        Cx::new(self.re * o.re + self.im * o.im, self.im * o.re - self.re * o.im)
    }

    pub fn scale(self, s: f32) -> Cx {
        Cx::new(self.re * s, self.im * s)
    }

    pub fn norm2(self) -> f32 {
        self.re * self.re + self.im * self.im
    }
}

/// Преобразование на месте; длина — степень двойки ≥ 2. Нормировка не делается:
/// обратный вызов нормирует сам.
pub(crate) fn transform(data: &mut [Cx], inverse: bool) {
    let n = data.len();
    assert!(n.is_power_of_two() && n >= 2);
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            data.swap(i, j);
        }
    }
    let sign = if inverse { 1.0f32 } else { -1.0f32 };
    let mut len = 2;
    while len <= n {
        let ang = sign * std::f32::consts::TAU / len as f32;
        let w_step = Cx::new(ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let mut w = Cx::new(1.0, 0.0);
            let half = len / 2;
            for k in 0..half {
                let a = data[start + k];
                let b = data[start + k + half].mul(w);
                data[start + k] = Cx::new(a.re + b.re, a.im + b.im);
                data[start + k + half] = Cx::new(a.re - b.re, a.im - b.im);
                w = w.mul(w_step);
            }
        }
        len *= 2;
    }
}

pub(crate) fn fft(data: &mut [Cx]) {
    transform(data, false);
}

pub(crate) fn ifft(data: &mut [Cx]) {
    let s = 1.0 / data.len() as f32;
    transform(data, true);
    for x in data.iter_mut() {
        *x = x.scale(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_dft(input: &[f32]) -> Vec<Cx> {
        let n = input.len();
        (0..n)
            .map(|k| {
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (t, &x) in input.iter().enumerate() {
                    let ang = -std::f32::consts::TAU * k as f32 * t as f32 / n as f32;
                    re += x * ang.cos();
                    im += x * ang.sin();
                }
                Cx::new(re, im)
            })
            .collect()
    }

    #[test]
    fn matches_naive_dft_and_roundtrip() {
        // Детерминированный LCG вместо крейта rand (P2).
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
        };
        let n = 32;
        let signal: Vec<f32> = (0..n).map(|_| next()).collect();
        let mut buf: Vec<Cx> = signal.iter().map(|&x| Cx::new(x, 0.0)).collect();
        fft(&mut buf);
        let expected = naive_dft(&signal);
        for (got, want) in buf.iter().zip(expected) {
            assert!((got.re - want.re).abs() < 1e-3, "re {} vs {}", got.re, want.re);
            assert!((got.im - want.im).abs() < 1e-3, "im {} vs {}", got.im, want.im);
        }
        ifft(&mut buf);
        for (got, &want) in buf.iter().zip(&signal) {
            assert!((got.re - want).abs() < 1e-4);
            assert!(got.im.abs() < 1e-4);
        }
    }

    #[test]
    fn impulse_gives_flat_spectrum() {
        let mut buf = vec![Cx::default(); 64];
        buf[0] = Cx::new(1.0, 0.0);
        fft(&mut buf);
        for x in &buf {
            assert!((x.re - 1.0).abs() < 1e-5);
            assert!(x.im.abs() < 1e-5);
        }
    }
}
