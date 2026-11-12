//! Timing samples and their summary (median, 90th percentile).

use std::time::{Duration, Instant};

use serde::Serialize;

/// Wall-clock milliseconds since `start`.
pub(crate) fn ms_since(start: Instant) -> f64 {
    as_ms(start.elapsed())
}

pub(crate) fn as_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Run `f` and return its result with its wall-clock time in ms.
pub(crate) fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let out = f();
    (out, ms_since(start))
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Summary {
    pub n: usize,
    pub median_ms: f64,
    pub p90_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
}

/// Nearest-rank percentile over a sorted slice (`q` in 0..=1).
fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub(crate) fn summarize(samples: &[f64]) -> Summary {
    if samples.is_empty() {
        return Summary::default();
    }
    let mut s = samples.to_vec();
    s.sort_by(f64::total_cmp);
    let median = if s.len() % 2 == 1 {
        s[s.len() / 2]
    } else {
        (s[s.len() / 2 - 1] + s[s.len() / 2]) / 2.0
    };
    Summary {
        n: s.len(),
        median_ms: median,
        p90_ms: percentile(&s, 0.9),
        min_ms: s[0],
        max_ms: s[s.len() - 1],
        mean_ms: s.iter().sum::<f64>() / s.len() as f64,
    }
}

/// Median of a slice of plain numbers (rates, not times).
pub(crate) fn median(xs: &[f64]) -> f64 {
    summarize(xs).median_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_and_p90() {
        let s = summarize(&[5.0, 1.0, 3.0, 2.0, 4.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
        assert_eq!(s.n, 10);
        assert!((s.median_ms - 5.5).abs() < 1e-9);
        assert!((s.p90_ms - 9.0).abs() < 1e-9);
        assert!((s.min_ms - 1.0).abs() < 1e-9);
    }

    #[test]
    fn empty_is_default() {
        assert_eq!(summarize(&[]).n, 0);
    }
}
