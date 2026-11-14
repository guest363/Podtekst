pub(super) const MIN_UNIT_SEC: f64 = 0.4;

pub(super) fn bounds(start: f64, end: f64) -> Vec<(f64, f64)> {
    const UNIT_SEC: f64 = 1.8;
    const MAX_UNIT_SEC: f64 = 2.4;
    const MAX_UNITS: usize = 8;
    let duration = (end - start).max(0.0);
    if duration < MIN_UNIT_SEC {
        return Vec::new();
    }
    if duration <= MAX_UNIT_SEC {
        return vec![(start, end)];
    }
    let windows = (((duration - UNIT_SEC) / 0.9).ceil() as usize + 1).clamp(2, MAX_UNITS);
    let step = ((duration - UNIT_SEC) / (windows - 1) as f64).max(0.9);
    (0..windows)
        .map(|window| {
            let from = start + window as f64 * step;
            (from, (from + UNIT_SEC).min(end))
        })
        .take_while(|(from, to)| to - from >= MIN_UNIT_SEC)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_window_preserves_full_short_utterance() {
        assert_eq!(bounds(0.0, 2.4), vec![(0.0, 2.4)]);
    }
}
