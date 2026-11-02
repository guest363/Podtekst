use super::delay;

const MAX_DELAY: usize = 8000;
const ECHO_MIN_PROMINENCE: f32 = 4.5;

// Зачем: живая донастройка AEC вслепую невозможна — сначала цифры по реальным
// стерео-записям (задержка/дрейф/double-talk). Только тест-харнесс, в прод-коде
// ни строки. Выход — одни числа, без аудио.
#[test]
fn live_corpus_diagnostics() {
    let Ok(path) = std::env::var("MR_DIAG_WAV") else {
        eprintln!("MR_DIAG_WAV не задан — пропуск");
        return;
    };
    let mut reader = hound::WavReader::open(&path).expect("WAV читается");
    let spec = reader.spec();
    assert_eq!(spec.channels, 2, "нужно стерео (микрофон / система)");
    assert_eq!(spec.sample_rate, 16000);
    let (mut mic, mut reference) = (Vec::new(), Vec::new());
    for (i, s) in reader.samples::<i16>().enumerate() {
        let v = s.expect("сэмплы читаются") as f32 / 32767.0;
        if i % 2 == 0 {
            mic.push(v);
        } else {
            reference.push(v);
        }
    }
    let n = mic.len().min(reference.len());
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    println!(
        "файл: {:.0} с, RMS микрофон={:.4} система={:.4}",
        n as f32 / 16000.0,
        rms(&mic[..n]),
        rms(&reference[..n])
    );

    // Задержка и уверенность по окнам 30 с на всей длине: тренд = дрейф часов.
    let win = 30 * 16000;
    let mut delays = vec![];
    let mut start = 0;
    let mut windows = 0;
    while start + delay::MIN_WINDOW_SAMPLES <= n {
        let end = (start + win).min(n);
        let (d, prom) = delay::estimate(&mic[start..end], &reference[start..end], MAX_DELAY)
            .map(|(d, p)| (d as i64, p))
            .unwrap_or((-1, 0.0));
        println!("окно {:4.0} с: задержка={d} проминентность={prom:.1}", start as f32 / 16000.0);
        windows += 1;
        if prom >= ECHO_MIN_PROMINENCE {
            delays.push(d);
        }
        if end == n {
            break;
        }
        start = end;
    }
    if delays.is_empty() {
        println!("уверенных окон нет: эхо ниже порога детекта на всей записи");
    } else {
        delays.sort();
        println!(
            "уверенных окон {}/{} задержка мин={} мед={} макс={} (разброс — джиттер/дрейф)",
            delays.len(),
            windows,
            delays[0],
            delays[delays.len() / 2],
            delays[delays.len() - 1],
        );
    }

    // Покадровая раскладка речи: чьи голоса и сколько одновременной речи.
    let frame = 320;
    let mut frames = vec![];
    for t in (0..n).step_by(frame) {
        let e = (t + frame).min(n);
        let pm: f32 = mic[t..e].iter().map(|v| v * v).sum::<f32>() / (e - t) as f32;
        let pr: f32 = reference[t..e].iter().map(|v| v * v).sum::<f32>() / (e - t) as f32;
        frames.push((pm.sqrt(), pr.sqrt()));
    }
    let floor = |idx: usize| {
        let mut v: Vec<f32> = frames.iter().map(|f| if idx == 0 { f.0 } else { f.1 }).collect();
        v.sort_by(f32::total_cmp);
        v[v.len() / 10]
    };
    let (tm, tr) = ((floor(0) * 4.0).max(0.01), (floor(1) * 4.0).max(0.01));
    let (mut sil, mut far, mut near, mut dbl) = (0usize, 0usize, 0usize, 0usize);
    let mut echo_ratios = vec![];
    let mut far_mic = vec![];
    for &(rm, rr) in &frames {
        match (rm > tm, rr > tr) {
            (false, false) => sil += 1,
            (false, true) => {
                far += 1;
                echo_ratios.push(rm * rm / (rr * rr).max(1e-12));
                far_mic.push(rm);
            }
            (true, false) => near += 1,
            (true, true) => dbl += 1,
        }
    }
    let total = (sil + far + near + dbl).max(1) as f32;
    echo_ratios.sort_by(f32::total_cmp);
    let att = if echo_ratios.is_empty() {
        "—".to_string()
    } else {
        format!("{:.1} дБ", -10.0 * echo_ratios[echo_ratios.len() / 2].max(1e-9).log10())
    };
    println!(
        "кадры: тишина={:.0}% только дальний={:.0}% только ближний={:.0}% оба={:.0}%; затухание эха на far-only ~{att}",
        100.0 * sil as f32 / total,
        100.0 * far as f32 / total,
        100.0 * near as f32 / total,
        100.0 * dbl as f32 / total,
    );
    // Шумовой пол vs эхо: если эхо на уровне пола, AEC физически нечего снимать.
    far_mic.sort_by(f32::total_cmp);
    let far_med = far_mic.get(far_mic.len() / 2).copied().unwrap_or(0.0);
    println!(
        "пол RMS: микрофон={:.4} система={:.4}; медиана микрофона на far-only={:.4} (эхо+шум)",
        floor(0),
        floor(1),
        far_med,
    );
}
