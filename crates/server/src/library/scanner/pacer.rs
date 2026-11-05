//! Scan progress pacing (Folio Flair phase 5, B1).
//!
//! One [`ScanPacer`] per running scan, kept in [`ProgressPacers`] on
//! `AppState`. Every progress report goes through [`ProgressPacers::observe`],
//! which:
//!
//! - samples the counters into a sliding [`RATE_WINDOW`], so `files_per_sec`,
//!   `bytes_per_sec` and the ETA follow what the scan is doing *now*, not the
//!   whole-run average (a scan that spent its first minute on unchanged
//!   folders otherwise reports a rate nothing like its current one);
//! - computes `eta_ms` only once the window spans [`ETA_MIN_SPAN`] with at
//!   least [`ETA_MIN_SAMPLES`] samples, so the first seconds don't flash
//!   wild estimates;
//! - runs the phase clock: `phase_elapsed_ms` is the time since the current
//!   wire phase (`scanning`, `reconciling`, …) began;
//! - throttles: at most one `scan.progress` per [`EMIT_MIN_INTERVAL`] per
//!   scan, except a phase change, which always goes out at once;
//! - decides when the `scan_runs.stats` snapshot is written: every
//!   [`PERSIST_INTERVAL`] and on each phase change (finalize writes the end
//!   state itself), independent of how often events go out.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// Fewest milliseconds between two `scan.progress` events of one scan.
pub const EMIT_MIN_INTERVAL: Duration = Duration::from_millis(400);
/// How often the progress snapshot is written to `scan_runs.stats`.
pub const PERSIST_INTERVAL: Duration = Duration::from_secs(2);
/// The span of samples the recent rate is computed over.
pub const RATE_WINDOW: Duration = Duration::from_secs(10);
/// The shortest sample span that yields a rate at all.
pub const RATE_MIN_SPAN: Duration = Duration::from_secs(1);
/// The shortest sample span that yields an ETA.
pub const ETA_MIN_SPAN: Duration = Duration::from_secs(3);
/// The fewest samples that yield an ETA.
pub const ETA_MIN_SAMPLES: usize = 3;
/// A pacer not touched for this long belongs to a run that never reached
/// finalize (a crashed task); it is dropped on the next registration.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// One reading of a scan's monotonic counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sample {
    /// Work units done (`ProgressState::completed`).
    pub completed: u64,
    /// Files with an outcome (added + updated + unchanged + skipped + dup).
    pub files: u64,
    pub bytes: u64,
}

/// Sliding window of `(when, sample)` readings.
#[derive(Debug, Default)]
pub struct RateWindow {
    samples: VecDeque<(Instant, Sample)>,
}

/// Rates over the window, per second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub completed_per_sec: f64,
    pub files_per_sec: f64,
    pub bytes_per_sec: f64,
    pub span: Duration,
    pub samples: usize,
}

impl RateWindow {
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Add a reading and drop the ones that fell out of [`RATE_WINDOW`]. The
    /// newest reading older than the window is kept as the anchor, so the
    /// span stays close to the full window instead of shrinking to the
    /// time since the oldest in-window reading.
    pub fn push(&mut self, at: Instant, sample: Sample) {
        self.samples.push_back((at, sample));
        while self.samples.len() > 2
            && self
                .samples
                .get(1)
                .is_some_and(|(t, _)| at.duration_since(*t) >= RATE_WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// Rates between the oldest and newest reading, once they span at least
    /// [`RATE_MIN_SPAN`].
    pub fn rates(&self) -> Option<Rates> {
        let (t0, first) = self.samples.front()?;
        let (t1, last) = self.samples.back()?;
        let span = t1.duration_since(*t0);
        if span < RATE_MIN_SPAN {
            return None;
        }
        let secs = span.as_secs_f64();
        let per = |a: u64, b: u64| b.saturating_sub(a) as f64 / secs;
        Some(Rates {
            completed_per_sec: per(first.completed, last.completed),
            files_per_sec: per(first.files, last.files),
            bytes_per_sec: per(first.bytes, last.bytes),
            span,
            samples: self.samples.len(),
        })
    }
}

/// `remaining ÷ rate` in milliseconds, once the rate is stable (see
/// [`ETA_MIN_SPAN`] / [`ETA_MIN_SAMPLES`]). `None` while settling, when the
/// scan is stalled (rate 0) or nothing remains.
pub fn eta_ms(rates: Option<Rates>, completed: u64, total: u64) -> Option<u64> {
    let r = rates?;
    if r.span < ETA_MIN_SPAN || r.samples < ETA_MIN_SAMPLES || r.completed_per_sec <= 0.0 {
        return None;
    }
    let remaining = total.saturating_sub(completed);
    if remaining == 0 {
        return None;
    }
    Some((remaining as f64 / r.completed_per_sec * 1000.0).round() as u64)
}

/// What one paced report should carry.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub phase_elapsed_ms: u64,
    /// Recent-window rates; `None` until the window spans [`RATE_MIN_SPAN`]
    /// (the caller falls back to the whole-run average).
    pub files_per_sec: Option<f64>,
    pub bytes_per_sec: Option<f64>,
    pub eta_ms: Option<u64>,
    pub batch_id: Option<Uuid>,
    pub phase_changed: bool,
    /// Write the `scan_runs.stats` snapshot with this report.
    pub persist: bool,
}

#[derive(Debug)]
struct ScanPacer {
    batch_id: Option<Uuid>,
    phase: Option<&'static str>,
    phase_started: Instant,
    last_emit: Option<Instant>,
    last_persist: Option<Instant>,
    window: RateWindow,
    touched: Instant,
}

impl ScanPacer {
    fn new(batch_id: Option<Uuid>, now: Instant) -> Self {
        Self {
            batch_id,
            phase: None,
            phase_started: now,
            last_emit: None,
            last_persist: None,
            window: RateWindow::default(),
            touched: now,
        }
    }

    fn observe(
        &mut self,
        phase: &'static str,
        sample: Sample,
        total: u64,
        persist_allowed: bool,
        now: Instant,
    ) -> Option<Frame> {
        self.touched = now;
        let phase_changed = self.phase != Some(phase);
        if phase_changed {
            self.phase = Some(phase);
            self.phase_started = now;
            // Rates are per phase: planning's counters say nothing about
            // how fast folders will scan.
            self.window.clear();
        }
        self.window.push(now, sample);
        if !phase_changed
            && self
                .last_emit
                .is_some_and(|t| now.duration_since(t) < EMIT_MIN_INTERVAL)
        {
            return None;
        }
        self.last_emit = Some(now);
        let persist = persist_allowed
            && (phase_changed
                || self
                    .last_persist
                    .is_none_or(|t| now.duration_since(t) >= PERSIST_INTERVAL));
        if persist {
            self.last_persist = Some(now);
        }
        let rates = self.window.rates();
        Some(Frame {
            phase_elapsed_ms: now.duration_since(self.phase_started).as_millis() as u64,
            files_per_sec: rates.map(|r| r.files_per_sec),
            bytes_per_sec: rates.map(|r| r.bytes_per_sec),
            eta_ms: eta_ms(rates, sample.completed, total),
            batch_id: self.batch_id,
            phase_changed,
            persist,
        })
    }
}

/// Every running scan's pacer, keyed by scan id.
#[derive(Debug, Default)]
pub struct ProgressPacers {
    inner: Mutex<HashMap<Uuid, ScanPacer>>,
}

impl ProgressPacers {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, ScanPacer>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A scan run opened; remember its batch for `scan.progress`.
    pub fn register(&self, scan_id: Uuid, batch_id: Option<Uuid>) {
        let now = Instant::now();
        let mut map = self.lock();
        map.retain(|_, p| now.duration_since(p.touched) < STALE_AFTER);
        map.insert(scan_id, ScanPacer::new(batch_id, now));
    }

    /// The run finalized; drop its pacer.
    pub fn finish(&self, scan_id: Uuid) {
        self.lock().remove(&scan_id);
    }

    /// Feed one progress report. `None` = throttled: send nothing. A scan
    /// that was never registered gets a pacer on first sight (no batch).
    /// `persist_allowed = false` (a snapshot write is still in flight) defers
    /// the write to a later report.
    pub fn observe(
        &self,
        scan_id: Uuid,
        phase: &'static str,
        sample: Sample,
        total: u64,
        persist_allowed: bool,
    ) -> Option<Frame> {
        let now = Instant::now();
        self.lock()
            .entry(scan_id)
            .or_insert_with(|| ScanPacer::new(None, now))
            .observe(phase, sample, total, persist_allowed, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(completed: u64, files: u64, bytes: u64) -> Sample {
        Sample {
            completed,
            files,
            bytes,
        }
    }

    #[test]
    fn rate_follows_the_recent_window_not_the_whole_run() {
        let t0 = Instant::now();
        let mut w = RateWindow::default();
        // 20 s at 1 file/s, then 10 s at 10 files/s.
        let mut files = 0;
        for sec in 0..=20u64 {
            files = sec;
            w.push(t0 + Duration::from_secs(sec), s(files, files, files * 100));
        }
        for sec in 21..=30u64 {
            files += 10;
            w.push(t0 + Duration::from_secs(sec), s(files, files, files * 100));
        }
        let r = w.rates().expect("window spans 10s");
        // Whole-run average would be 120/30 = 4/s; the window sees 10/s.
        assert!((r.files_per_sec - 10.0).abs() < 1e-9, "{r:?}");
        assert!((r.bytes_per_sec - 1000.0).abs() < 1e-6, "{r:?}");
        assert_eq!(r.span, Duration::from_secs(10));
    }

    #[test]
    fn no_rate_until_the_window_spans_a_second() {
        let t0 = Instant::now();
        let mut w = RateWindow::default();
        w.push(t0, s(0, 0, 0));
        w.push(t0 + Duration::from_millis(500), s(5, 5, 0));
        assert!(w.rates().is_none());
        w.push(t0 + Duration::from_millis(1000), s(10, 10, 0));
        assert!((w.rates().unwrap().files_per_sec - 10.0).abs() < 1e-9);
    }

    #[test]
    fn eta_waits_for_a_stable_rate_then_divides_remaining_by_it() {
        let t0 = Instant::now();
        let mut w = RateWindow::default();
        w.push(t0, s(0, 0, 0));
        w.push(t0 + Duration::from_secs(1), s(5, 5, 0));
        w.push(t0 + Duration::from_secs(2), s(10, 10, 0));
        // 2 s span: too short.
        assert_eq!(eta_ms(w.rates(), 10, 100), None);
        w.push(t0 + Duration::from_secs(3), s(15, 15, 0));
        // 5 units/s, 85 remaining → 17 s.
        assert_eq!(eta_ms(w.rates(), 15, 100), Some(17_000));
        // Nothing left, or a stall, gives no ETA.
        assert_eq!(eta_ms(w.rates(), 100, 100), None);
        let mut stalled = RateWindow::default();
        for sec in 0..5 {
            stalled.push(t0 + Duration::from_secs(sec), s(7, 7, 0));
        }
        assert_eq!(eta_ms(stalled.rates(), 7, 100), None);
    }

    #[test]
    fn emits_are_throttled_but_phase_changes_go_out_at_once() {
        let t0 = Instant::now();
        let mut p = ScanPacer::new(Some(Uuid::nil()), t0);
        let f = p
            .observe("scanning", s(0, 0, 0), 10, true, t0)
            .expect("first report");
        assert!(f.phase_changed && f.persist);
        assert_eq!(f.batch_id, Some(Uuid::nil()));
        // 100 ms later: throttled.
        assert!(
            p.observe(
                "scanning",
                s(1, 1, 0),
                10,
                true,
                t0 + Duration::from_millis(100)
            )
            .is_none()
        );
        // A phase change 200 ms in is never throttled, and persists.
        let f = p
            .observe(
                "reconciling",
                s(1, 1, 0),
                10,
                true,
                t0 + Duration::from_millis(200),
            )
            .expect("phase change");
        assert!(f.phase_changed && f.persist);
        assert_eq!(f.phase_elapsed_ms, 0);
        // 400 ms after that emit: allowed, phase clock running, no persist
        // (the phase change wrote 0.4 s ago).
        let f = p
            .observe(
                "reconciling",
                s(1, 1, 0),
                10,
                true,
                t0 + Duration::from_millis(600),
            )
            .expect("past the throttle");
        assert_eq!(f.phase_elapsed_ms, 400);
        assert!(!f.persist && !f.phase_changed);
    }

    #[test]
    fn snapshot_writes_follow_their_own_cadence() {
        let t0 = Instant::now();
        let mut p = ScanPacer::new(None, t0);
        let mut persisted = Vec::new();
        for i in 0..=12u64 {
            let at = t0 + Duration::from_millis(500 * i);
            if let Some(f) = p.observe("scanning", s(i, i, 0), 100, true, at)
                && f.persist
            {
                persisted.push(i * 500);
            }
        }
        // Every report emits (500 ms apart), but writes land every 2 s.
        assert_eq!(persisted, vec![0, 2000, 4000, 6000]);

        // A write still in flight defers the next one rather than dropping
        // the cadence.
        let at = t0 + Duration::from_millis(8000);
        let f = p.observe("scanning", s(20, 20, 0), 100, false, at).unwrap();
        assert!(!f.persist);
        let f = p
            .observe(
                "scanning",
                s(21, 21, 0),
                100,
                true,
                at + Duration::from_millis(500),
            )
            .unwrap();
        assert!(f.persist);
    }

    #[test]
    fn registry_tracks_batches_and_forgets_finished_scans() {
        let pacers = ProgressPacers::default();
        let scan = Uuid::now_v7();
        let batch = Uuid::now_v7();
        pacers.register(scan, Some(batch));
        let f = pacers
            .observe(scan, "planning", Sample::default(), 1, true)
            .unwrap();
        assert_eq!(f.batch_id, Some(batch));
        pacers.finish(scan);
        let f = pacers
            .observe(scan, "planning", Sample::default(), 1, true)
            .unwrap();
        assert_eq!(
            f.batch_id, None,
            "an unregistered scan starts a fresh pacer"
        );
    }
}
