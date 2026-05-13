//! Per-scan counters returned to the API and persisted to `scan_runs.stats`.
//!
//! Library Scanner v1, spec §B5 (telemetry shape) + §10 (counts of malformed /
//! encrypted feed library health rather than scan failure).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScanStats {
    pub files_seen: u64,
    pub files_skipped: u64,
    pub files_added: u64,
    pub files_updated: u64,
    pub files_unchanged: u64,
    pub files_malformed: u64,
    pub files_encrypted: u64,
    /// Files whose content hash already belongs to another issue row at a
    /// different `file_path`. Surfaces in the History tab's Results column
    /// and emits a `DuplicateContent` health issue per occurrence so
    /// admins can pick which copy to keep. Honors the spec §6 "dedupe by
    /// content" semantics; the proper alias-table follow-up (spec §10.1)
    /// is still pending, but this counter at least makes the rejections
    /// visible instead of silently rolling back the chunk.
    pub files_duplicate: u64,
    pub series_created: u64,
    pub series_skipped_unchanged: u64,
    pub series_removed: u64,
    pub issues_removed: u64,
    pub issues_restored: u64,
    pub thumbs_generated: u64,
    pub thumbs_failed: u64,
    pub elapsed_ms: u64,
    /// Wall-clock time spent in **serial** scanner phases (one observer at a
    /// time): `plan`, `enumerate`, `reconcile`, `thumbnail_enqueue`, plus the
    /// `process` umbrella that wraps the parallel folder-processing loop.
    /// Numbers here approximate "wall contribution"; sum across keys is
    /// roughly the non-parallel portion of `elapsed_ms`.
    pub phase_timings_ms: BTreeMap<String, u64>,
    /// **Summed across parallel workers** for phases recorded inside
    /// `process_planned_folder` / `ingest_one_with_fingerprint`: `hash`,
    /// `archive_parse`, `page_probe`, `identity`, `db_write`,
    /// `metadata_rollup`. Numbers here exceed wall time by roughly
    /// `scan_worker_count`× — divide by `parallel_workers` (also recorded
    /// here when ≥1) to estimate wall contribution. Made explicit because
    /// the previous shape mixed both into a single map and made
    /// `db_write: 44_143ms` look like the dominant cost on a 15s wall scan.
    /// See `docs/dev/scanner-perf.md` for the methodology that surfaced
    /// this.
    #[serde(default)]
    pub parallel_phase_timings_ms: BTreeMap<String, u64>,
    /// Concurrency factor that produced `parallel_phase_timings_ms`. Equals
    /// `state.cfg().scan_worker_count` for library scans; 1 for series/issue
    /// narrow scans. `0` if no parallel work fired.
    #[serde(default)]
    pub parallel_workers: u32,
    pub bytes_hashed: u64,
    pub files_per_sec: Option<f64>,
    pub bytes_per_sec: Option<f64>,
}

impl ScanStats {
    /// Record time in a **serial** phase (single observer; wall-clock).
    pub fn record_phase(&mut self, phase: impl Into<String>, elapsed: Duration) {
        let ms = elapsed.as_millis() as u64;
        if ms == 0 {
            return;
        }
        *self.phase_timings_ms.entry(phase.into()).or_insert(0) += ms;
    }

    /// Record time in a **parallel** phase (one of N workers; numbers here
    /// sum across workers when local stats merge into the global). Use
    /// from inside `process_planned_folder` / `ingest_one_with_fingerprint`.
    pub fn record_phase_parallel(&mut self, phase: impl Into<String>, elapsed: Duration) {
        let ms = elapsed.as_millis() as u64;
        if ms == 0 {
            return;
        }
        *self
            .parallel_phase_timings_ms
            .entry(phase.into())
            .or_insert(0) += ms;
    }

    /// Set how many workers contributed to `parallel_phase_timings_ms`. Call
    /// once at scan start so doc readers can derive wall ≈ summed/workers.
    pub fn set_parallel_workers(&mut self, n: u32) {
        self.parallel_workers = n;
    }

    pub fn record_bytes_hashed(&mut self, bytes: u64) {
        self.bytes_hashed = self.bytes_hashed.saturating_add(bytes);
    }

    pub fn finalize_rates(&mut self) {
        if self.elapsed_ms == 0 {
            self.files_per_sec = None;
            self.bytes_per_sec = None;
            return;
        }
        let seconds = self.elapsed_ms as f64 / 1000.0;
        let processed = self
            .files_added
            .saturating_add(self.files_updated)
            .saturating_add(self.files_unchanged)
            .saturating_add(self.files_skipped)
            .saturating_add(self.files_duplicate);
        self.files_per_sec = Some(processed as f64 / seconds);
        self.bytes_per_sec = Some(self.bytes_hashed as f64 / seconds);
    }

    pub fn merge(&mut self, other: ScanStats) {
        self.files_seen += other.files_seen;
        self.files_skipped += other.files_skipped;
        self.files_added += other.files_added;
        self.files_updated += other.files_updated;
        self.files_unchanged += other.files_unchanged;
        self.files_malformed += other.files_malformed;
        self.files_encrypted += other.files_encrypted;
        self.files_duplicate += other.files_duplicate;
        self.series_created += other.series_created;
        self.series_skipped_unchanged += other.series_skipped_unchanged;
        self.series_removed += other.series_removed;
        self.issues_removed += other.issues_removed;
        self.issues_restored += other.issues_restored;
        self.thumbs_generated += other.thumbs_generated;
        self.thumbs_failed += other.thumbs_failed;
        self.elapsed_ms = self.elapsed_ms.max(other.elapsed_ms);
        self.bytes_hashed = self.bytes_hashed.saturating_add(other.bytes_hashed);
        for (phase, ms) in other.phase_timings_ms {
            *self.phase_timings_ms.entry(phase).or_insert(0) += ms;
        }
        for (phase, ms) in other.parallel_phase_timings_ms {
            *self.parallel_phase_timings_ms.entry(phase).or_insert(0) += ms;
        }
        // parallel_workers is set once at scan start on the global stats;
        // local worker stats start at 0. Take max so a non-zero global
        // wins over a zero local.
        self.parallel_workers = self.parallel_workers.max(other.parallel_workers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_phase_serial_lives_in_phase_timings_ms() {
        let mut s = ScanStats::default();
        s.record_phase("plan", Duration::from_millis(7));
        s.record_phase("plan", Duration::from_millis(3));
        assert_eq!(s.phase_timings_ms.get("plan").copied(), Some(10));
        assert!(s.parallel_phase_timings_ms.is_empty());
    }

    #[test]
    fn record_phase_parallel_lives_in_parallel_map() {
        let mut s = ScanStats::default();
        s.record_phase_parallel("hash", Duration::from_millis(50));
        s.record_phase_parallel("hash", Duration::from_millis(50));
        assert_eq!(s.parallel_phase_timings_ms.get("hash").copied(), Some(100),);
        assert!(s.phase_timings_ms.is_empty());
    }

    #[test]
    fn merge_combines_both_maps() {
        let mut a = ScanStats::default();
        a.record_phase("plan", Duration::from_millis(10));
        a.record_phase_parallel("db_write", Duration::from_millis(100));
        a.set_parallel_workers(4);

        let mut b = ScanStats::default();
        b.record_phase_parallel("db_write", Duration::from_millis(200));
        b.record_phase_parallel("hash", Duration::from_millis(50));

        a.merge(b);
        assert_eq!(a.phase_timings_ms.get("plan").copied(), Some(10));
        assert_eq!(
            a.parallel_phase_timings_ms.get("db_write").copied(),
            Some(300),
        );
        assert_eq!(a.parallel_phase_timings_ms.get("hash").copied(), Some(50),);
        assert_eq!(a.parallel_workers, 4);
    }

    #[test]
    fn zero_duration_does_not_create_entry() {
        let mut s = ScanStats::default();
        s.record_phase("plan", Duration::from_micros(500));
        s.record_phase_parallel("hash", Duration::from_micros(500));
        assert!(s.phase_timings_ms.is_empty());
        assert!(s.parallel_phase_timings_ms.is_empty());
    }
}
