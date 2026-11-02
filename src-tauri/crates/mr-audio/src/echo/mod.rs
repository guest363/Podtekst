mod delay;
mod fft;
mod filter;

use filter::EchoFilter;

/// 500 мс: покрывает loopback (10–50 мс) и Bluetooth-колонки (100–300 мс).
const MAX_DELAY: usize = 8000;
// Гейт закрывает только случай тихого системного канала (экономия DSP):
// речеподобный несвязанный звук неотличим от слабого эха по PHAT (пол
// 5.4, слабое эхо 5.2+), защита сценария «наушники» — мёртвая зона
// когерентности в фильтре (несвязанные каналы дают coh ~0.16 ниже 0.3 →
// шаг почти нулевой, артефакт не глубже −25 дБ).
const ECHO_MIN_PROMINENCE: f32 = 4.5;
/// Полка шума в критерии «только дальний собеседник»: ниже неё блок
/// считается тишиной и в ERLE не попадает.
const POWER_FLOOR: f32 = 1e-7;

#[derive(Debug)]
pub struct EchoReport {
    /// Эхо обнаружено и канал обработан.
    pub echo_windows: usize,
    /// Медиана ERLE по блокам «только дальний собеседник», дБ.
    pub erle_db: Option<f32>,
}

/// Чистит канал микрофона (`mic`) по референсу системного звука.
/// Оба канала — моно 16 кГц из стерео-сплита записи (mic — левый).
/// Аудио без эха (наушники, паузы системного звука) остаётся практически
/// без изменений: снимается только DC/гул ниже ~75 Гц.
pub fn suppress_echo(mic: &mut [f32], reference: &[f32], sample_rate: u32) -> EchoReport {
    let n = mic.len().min(reference.len());
    if sample_rate != 16000 || n < delay::MIN_WINDOW_SAMPLES {
        return EchoReport {
            echo_windows: 0,
            erle_db: None,
        };
    }
    // Гейт эха: максимум проминентности по окнам 10 с (до 3 минут).
    // Длинное усреднение размазывает пик, если задержка дрейфует; в
    // отдельном же окне она локально стабильна. Задержка из оценки не
    // применяется: эхопуть до 320 мс фильтр покрывает сам; выравнивание
    // нужно лишь для задержек больше ёмкости фильтра (Bluetooth-колонки).
    let mut echo_present = false;
    let window_len = 10 * 16000;
    let probe_end = n.min(180 * 16000);
    let mut w = 0usize;
    while w + delay::MIN_WINDOW_SAMPLES <= probe_end {
        let w1 = (w + window_len).min(probe_end);
        if let Some((_, prom)) = delay::estimate(&mic[w..w1], &reference[w..w1], MAX_DELAY) {
            if prom >= ECHO_MIN_PROMINENCE {
                echo_present = true;
                break;
            }
        }
        w = w1;
    }
    dc_block(mic);

    let echo_windows = usize::from(echo_present);
    let mut erle: Vec<f32> = Vec::new();

    // Обработка независимыми чанками по 10 с со свежим фильтром в каждом:
    // в пределах чанка адаптивный фильтр устойчив (сходится за ~1.5 с
    // дальней речи), а блуждание на двойном разговоре не успевает
    // накопиться и не переходит между чанками. Эхопуть статичен, каждый
    // чанк переучивает его заново.
    if echo_present {
        // Проход 1 — прогрев на копии канала: к выводу фильтр уже сошёлся,
        // и первые секунды записи не выходят с недавшим фильтром.
        let mut line = DelayLine::new(reference);
        let mut filter = EchoFilter::new();
        let mut scratch = vec![0.0f32; filter::BLOCK];
        let mut t = 0usize;
        while t + filter::BLOCK <= n {
            let mut ref_block = vec![0.0f32; filter::BLOCK];
            line.pull(0, t, &mut ref_block);
            scratch.copy_from_slice(&mic[t..t + filter::BLOCK]);
            filter.process(&mut scratch, &ref_block, 1.0);
            t += filter::BLOCK;
        }
        // Проход 2 — подавление в живом канале.
        let mut line = DelayLine::new(reference);
        let mut t = 0usize;
        while t + filter::BLOCK <= n {
            let mut ref_block = vec![0.0f32; filter::BLOCK];
            line.pull(0, t, &mut ref_block);
            let stats = filter.process(&mut mic[t..t + filter::BLOCK], &ref_block, 1.0);
            if stats.echo_power > POWER_FLOOR && stats.mic_power <= 2.0 * stats.echo_power {
                erle.push(10.0 * (stats.mic_power / (stats.out_power + 1e-10)).log10());
            }
            t += filter::BLOCK;
        }
    }

    EchoReport {
        echo_windows,
        erle_db: median(&mut erle),
    }
}

/// DC-блокер (полюс 0.97 ≈ 75 Гц на 16 кГц): DC и гул рушат нормировку
/// мощности бинов в адаптивном фильтре.
fn dc_block(samples: &mut [f32]) {
    let (mut x_prev, mut y_prev) = (0.0f32, 0.0f32);
    for s in samples.iter_mut() {
        let x = *s;
        *s = x - x_prev + 0.97 * y_prev;
        x_prev = x;
        y_prev = *s;
    }
}

/// Потоковый доступ к DC-блокированному референсу с произвольной задержкой
/// 0..MAX_DELAY: копия всего канала на час записи — сотни МБ, кольцо — 33 КБ.
struct DelayLine<'a> {
    reference: &'a [f32],
    buf: Vec<f32>,
    pos: usize,
    produced: usize,
    x_prev: f32,
    y_prev: f32,
}

impl<'a> DelayLine<'a> {
    fn new(reference: &'a [f32]) -> Self {
        Self {
            reference,
            buf: vec![0.0; MAX_DELAY + filter::BLOCK],
            pos: 0,
            produced: 0,
            x_prev: 0.0,
            y_prev: 0.0,
        }
    }

    fn push_one(&mut self) {
        let s = self.reference.get(self.produced).copied().unwrap_or(0.0);
        let y = s - self.x_prev + 0.97 * self.y_prev;
        self.x_prev = s;
        self.y_prev = y;
        self.buf[self.pos] = y;
        self.pos = (self.pos + 1) % self.buf.len();
        self.produced += 1;
    }

    /// Заполняет `out` сэмплами референса с задержкой `delay` относительно
    /// позиции `t` обрабатываемого блока микрофона. Вызывается по блокам
    /// подряд; за пределами записи — тишина (системный канал короче микрофонного).
    fn pull(&mut self, delay: usize, t: usize, out: &mut [f32]) {
        let end = t + out.len();
        while self.produced < end {
            self.push_one();
        }
        for (j, slot) in out.iter_mut().enumerate() {
            // saturating_sub: до начала записи референса нет — тишина
            // (usize-вычитание в release молча заворачивалось бы).
            let i = (t + j).saturating_sub(delay);
            *slot = if i < self.produced {
                self.buf[i % self.buf.len()]
            } else {
                0.0
            };
        }
    }
}

fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_block_removes_offset_keeps_wave() {
        let mut s: Vec<f32> = (0..16000)
            .map(|i| 0.5 + (std::f32::consts::TAU * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        dc_block(&mut s);
        let tail_mean: f32 = s[8000..].iter().sum::<f32>() / 8000.0;
        assert!(tail_mean.abs() < 0.05, "среднее {tail_mean}");
        let amp = s[8000..].iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        assert!((amp - 1.0).abs() < 0.2, "амплитуда {amp}");
    }

    #[test]
    fn delay_line_returns_shifted_reference() {
        let reference: Vec<f32> = (0..30000).map(|i| (i as f32 * 0.1).sin()).collect();
        // Линия задержки прогоняет референс через DC-блокер — эталон считаем
        // тем же фильтром, проверяется именно кольцевая арифметика.
        let mut blocked = reference.clone();
        dc_block(&mut blocked);
        let mut line = DelayLine::new(&reference);
        let delay = 5000usize;
        let mut out = vec![0.0f32; filter::BLOCK];
        line.pull(delay, 6000, &mut out);
        for (j, &got) in out.iter().enumerate() {
            let want = blocked[6000 + j - delay];
            assert!((got - want).abs() < 1e-4, "j={j} {got} vs {want}");
        }
    }
}
