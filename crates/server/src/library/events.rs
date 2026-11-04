//! Scan progress event broadcast (spec §8.1).
//!
//! Library Scanner v1, Milestone 10.
//!
//! A single tokio `broadcast` channel lives in [`crate::state::AppState`].
//! The scanner emits events through [`Broadcaster::emit`]; WS subscribers
//! receive them via [`Broadcaster::subscribe`].
//!
//! Throttling (spec §14.3): `scan.series_updated` is the chatty one — a
//! 5000-series library can emit thousands per minute. We coalesce these
//! per-library to ≤10/sec by gating with a `last_emitted` timestamp map,
//! with new series (`created`) and changed ones throttled separately so a
//! rescan's stream of changes never hides the discoveries. `scan.progress`
//! is paced by the scanner itself (`scanner::pacer`), and `queue.activity`
//! is one aggregated event per flush (`jobs::activity`). Other event types
//! pass through immediately.
//!
//! Documented deferral: spec §8.1's per-user library-access filter
//! (subscribers only see events for libraries they have access to via
//! `library_user_access`). Today the WS endpoint is admin-only, so the
//! filter is implied; mixing with non-admin subscribers would need the
//! per-message ACL check.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use uuid::Uuid;

const CHANNEL_CAPACITY: usize = 1024;
const SERIES_UPDATED_THROTTLE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[expect(clippy::large_enum_variant)]
pub enum ScanEvent {
    #[serde(rename = "scan.started")]
    Started {
        library_id: Uuid,
        scan_id: Uuid,
        at: DateTime<Utc>,
        /// The "Scan all" batch this run belongs to (observability-split M8).
        /// `None` for single-library / series / issue scans. The live
        /// dashboard uses this to learn which libraries are members of a
        /// batch; subsequent `Progress` events are correlated by `library_id`.
        #[serde(skip_serializing_if = "Option::is_none")]
        batch_id: Option<Uuid>,
    },
    #[serde(rename = "scan.progress")]
    Progress {
        library_id: Uuid,
        scan_id: Uuid,
        kind: &'static str,
        phase: &'static str,
        unit: &'static str,
        completed: u64,
        total: u64,
        current_label: Option<String>,
        files_seen: u64,
        files_added: u64,
        files_updated: u64,
        files_unchanged: u64,
        files_skipped: u64,
        files_duplicate: u64,
        issues_removed: u64,
        health_issues: u64,
        series_scanned: u64,
        series_total: u64,
        series_skipped_unchanged: u64,
        files_total: u64,
        root_files: u64,
        empty_folders: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase_elapsed_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        files_per_sec: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        bytes_per_sec: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        active_workers: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        dirty_folders: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        skipped_folders: Option<u64>,
        /// Remaining work ÷ the recent completion rate. Absent until the
        /// rate has settled (a few seconds of samples).
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_ms: Option<u64>,
        /// The "Scan all" batch this run belongs to, as on `scan.started`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch_id: Option<Uuid>,
    },
    /// A folder's series was ingested (sent once the folder's files are in,
    /// so a new series already has issues). Throttled per library.
    #[serde(rename = "scan.series_updated")]
    SeriesUpdated {
        library_id: Uuid,
        series_id: Uuid,
        name: String,
        /// The scan created this series (vs. changed an existing one).
        #[serde(default)]
        created: bool,
        /// The series cover thumbnail (`/issues/{id}/pages/0/thumb`), when
        /// the series has an active issue to show.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cover_url: Option<String>,
    },
    #[serde(rename = "scan.health_issue")]
    HealthIssue {
        library_id: Uuid,
        scan_id: Uuid,
        kind: String,
        severity: String,
        path: Option<String>,
    },
    #[serde(rename = "scan.completed")]
    Completed {
        library_id: Uuid,
        scan_id: Uuid,
        added: u64,
        updated: u64,
        removed: u64,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        batch_id: Option<Uuid>,
        /// Scope of a narrow scan (post-audit T1.6): the series a
        /// series/issue-scoped run covered, and the issue for an
        /// issue-scoped run. Absent on library-wide scans. Lets a client
        /// waiting on *its* rescan (the metadata apply dialog) ignore
        /// unrelated completions in the same library.
        #[serde(skip_serializing_if = "Option::is_none")]
        series_id: Option<Uuid>,
        #[serde(skip_serializing_if = "Option::is_none")]
        issue_id: Option<String>,
    },
    #[serde(rename = "scan.failed")]
    Failed {
        library_id: Uuid,
        scan_id: Uuid,
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        batch_id: Option<Uuid>,
    },
    /// Thumbnail pipeline (M4): the post-scan worker started a job for this
    /// issue. Lets the admin Live scan view tick a "Thumbnails: X/Y" counter
    /// alongside the series counter.
    #[serde(rename = "thumbs.started")]
    ThumbsStarted {
        library_id: Uuid,
        issue_id: String,
        kind: String,
    },
    /// The job finished successfully. `pages` is the archive's page count
    /// as the job walked it (`1` for cover-only jobs). The cover is one of
    /// those pages, never an extra — the same number feeds
    /// `issue.page_count` reconciliation in the worker.
    #[serde(rename = "thumbs.completed")]
    ThumbsCompleted {
        library_id: Uuid,
        issue_id: String,
        kind: String,
        pages: u64,
    },
    /// The job ended with an error stamped on the issue row. Surfaces as a
    /// toast in the admin UI so admins know to look.
    #[serde(rename = "thumbs.failed")]
    ThumbsFailed {
        library_id: Uuid,
        issue_id: String,
        kind: String,
        error: String,
    },
    /// A metadata apply landed via the **DB-direct** (non-writeback) path,
    /// so the issue/series rows — covers, fields, notes — are now current.
    /// The writeback path signals completion via `scan.completed` after its
    /// rescan; this is the equivalent for unmigrated libraries so the match
    /// dialog can re-hydrate open tabs without a page refresh. `issue_id`
    /// is `None` for a series-scope apply.
    #[serde(rename = "metadata.applied")]
    MetadataApplied {
        library_id: Uuid,
        series_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        issue_id: Option<String>,
        /// The metadata run whose apply landed (post-audit T1.6). Clients
        /// that applied a run match on it instead of "any apply in this
        /// library".
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<Uuid>,
    },
    /// A metadata apply job failed (post-audit T1.6). Without it a failed
    /// apply was indistinguishable from a slow one: the dialog waited out
    /// its timeout and then claimed success.
    #[serde(rename = "metadata.failed")]
    MetadataFailed {
        library_id: Uuid,
        run_id: Uuid,
        series_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        issue_id: Option<String>,
        error: String,
    },
    /// A background backfill drain finished (audit B17). `kind` is
    /// `cover_phash` | `variant_cover`; `processed` is the count that landed
    /// (hashed / re-downloaded), `skipped` the residual rows that couldn't
    /// be (undecodable covers / dead provider URLs). Lets the admin
    /// dashboard card report the result of the now-async sweep.
    #[serde(rename = "backfill.completed")]
    BackfillCompleted {
        kind: String,
        processed: u64,
        skipped: u64,
    },
    /// Job lifecycle for the admin queue lanes (Flair phase 5 B3): what
    /// started and finished in each queue since the previous event. One
    /// event per ~500 ms flush, only when something changed — never one per
    /// job. See [`crate::jobs::activity`].
    #[serde(rename = "queue.activity")]
    QueueActivity {
        at: DateTime<Utc>,
        queues: Vec<crate::jobs::activity::QueueActivity>,
    },
}

impl ScanEvent {
    fn library_id(&self) -> Uuid {
        match self {
            Self::Started { library_id, .. }
            | Self::Progress { library_id, .. }
            | Self::SeriesUpdated { library_id, .. }
            | Self::HealthIssue { library_id, .. }
            | Self::Completed { library_id, .. }
            | Self::Failed { library_id, .. }
            | Self::ThumbsStarted { library_id, .. }
            | Self::ThumbsCompleted { library_id, .. }
            | Self::ThumbsFailed { library_id, .. }
            | Self::MetadataApplied { library_id, .. }
            | Self::MetadataFailed { library_id, .. } => *library_id,
            // Library-agnostic; only reached via the SeriesUpdated-guarded
            // throttle, so this arm is never hit at runtime.
            Self::BackfillCompleted { .. } | Self::QueueActivity { .. } => Uuid::nil(),
        }
    }
}

#[derive(Clone)]
pub struct Broadcaster {
    sender: broadcast::Sender<ScanEvent>,
    /// Keyed by `(library_id, created)`.
    series_updated_last: Arc<Mutex<HashMap<(Uuid, bool), Instant>>>,
}

impl Broadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            sender,
            series_updated_last: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ScanEvent> {
        self.sender.subscribe()
    }

    /// Would a `scan.series_updated` for this library (and `created`
    /// flag) pass the throttle right now? Lets the scanner skip the cover
    /// lookup for an event that would be dropped anyway.
    pub fn series_updated_due(&self, library_id: Uuid, created: bool) -> bool {
        let map = self.series_updated_last.lock().expect("throttle mutex");
        map.get(&(library_id, created))
            .is_none_or(|prev| prev.elapsed() >= SERIES_UPDATED_THROTTLE)
    }

    pub fn emit(&self, evt: ScanEvent) {
        // Throttle: only `SeriesUpdated` is rate-limited per-library.
        if let ScanEvent::SeriesUpdated { created, .. } = &evt {
            let key = (evt.library_id(), *created);
            let now = Instant::now();
            let mut map = self.series_updated_last.lock().expect("throttle mutex");
            if let Some(prev) = map.get(&key)
                && now.duration_since(*prev) < SERIES_UPDATED_THROTTLE
            {
                return; // Skip — within throttle window.
            }
            map.insert(key, now);
        }
        // Receivers may have dropped — that's fine (broadcast::Sender::send
        // returns Err when no receivers, which we ignore).
        let _ = self.sender.send(evt);
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::activity::{FinishedActivity, LaneCounts, QueueActivity};
    use serde_json::json;

    #[test]
    fn queue_activity_wire_shape() {
        let at = DateTime::parse_from_rfc3339("2026-11-04T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let evt = ScanEvent::QueueActivity {
            at,
            queues: vec![QueueActivity {
                queue: "post_scan_thumbs".into(),
                started: vec!["A".into()],
                started_count: 1,
                finished: vec![
                    FinishedActivity {
                        task_id: "B".into(),
                        ok: true,
                        duration_ms: 12,
                        error: None,
                    },
                    FinishedActivity {
                        task_id: "C".into(),
                        ok: false,
                        duration_ms: 40,
                        error: Some("boom".into()),
                    },
                ],
                finished_count: 2,
                counts: Some(LaneCounts {
                    waiting: 3,
                    scheduled: 1,
                    running: 2,
                }),
            }],
        };
        let v = serde_json::to_value(&evt).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "queue.activity",
                "at": "2026-11-04T10:00:00Z",
                "queues": [{
                    "queue": "post_scan_thumbs",
                    "started": ["A"],
                    "started_count": 1,
                    "finished": [
                        { "task_id": "B", "ok": true, "duration_ms": 12 },
                        { "task_id": "C", "ok": false, "duration_ms": 40, "error": "boom" },
                    ],
                    "finished_count": 2,
                    "counts": { "waiting": 3, "scheduled": 1, "running": 2 },
                }],
            })
        );
        // `ScanEvent` borrows `&'static str`s, so round-trip the payload.
        let back: Vec<QueueActivity> = serde_json::from_value(v["queues"].clone()).unwrap();
        assert_eq!(back[0].finished[1].error.as_deref(), Some("boom"));
    }

    #[test]
    fn series_updated_carries_created_and_cover() {
        let lib = Uuid::nil();
        let evt = ScanEvent::SeriesUpdated {
            library_id: lib,
            series_id: lib,
            name: "Saga".into(),
            created: true,
            cover_url: Some("/issues/abc/pages/0/thumb".into()),
        };
        let v = serde_json::to_value(&evt).unwrap();
        assert_eq!(v["type"], "scan.series_updated");
        assert_eq!(v["created"], true);
        assert_eq!(v["cover_url"], "/issues/abc/pages/0/thumb");
        // No cover: the key is omitted; an old payload without the new keys
        // still parses (created defaults to false).
        const OLD: &str = r#"{"type":"scan.series_updated","library_id":"00000000-0000-0000-0000-000000000000","series_id":"00000000-0000-0000-0000-000000000000","name":"Saga"}"#;
        match serde_json::from_str::<ScanEvent>(OLD).unwrap() {
            ScanEvent::SeriesUpdated {
                created, cover_url, ..
            } => {
                assert!(!created);
                assert!(cover_url.is_none());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn progress_carries_batch_and_eta_only_when_known() {
        let mk = |batch_id, eta_ms| ScanEvent::Progress {
            library_id: Uuid::nil(),
            scan_id: Uuid::nil(),
            kind: "library",
            phase: "scanning",
            unit: "work",
            completed: 1,
            total: 2,
            current_label: None,
            files_seen: 0,
            files_added: 0,
            files_updated: 0,
            files_unchanged: 0,
            files_skipped: 0,
            files_duplicate: 0,
            issues_removed: 0,
            health_issues: 0,
            series_scanned: 0,
            series_total: 0,
            series_skipped_unchanged: 0,
            files_total: 0,
            root_files: 0,
            empty_folders: 0,
            elapsed_ms: None,
            phase_elapsed_ms: Some(5),
            files_per_sec: None,
            bytes_per_sec: None,
            active_workers: None,
            dirty_folders: None,
            skipped_folders: None,
            eta_ms,
            batch_id,
        };
        let v = serde_json::to_value(mk(None, None)).unwrap();
        assert!(v.get("batch_id").is_none() && v.get("eta_ms").is_none());
        assert_eq!(v["phase_elapsed_ms"], 5);
        let b = Uuid::now_v7();
        let v = serde_json::to_value(mk(Some(b), Some(1500))).unwrap();
        assert_eq!(v["batch_id"], b.to_string());
        assert_eq!(v["eta_ms"], 1500);
    }

    #[test]
    fn created_and_changed_series_throttle_separately() {
        let b = Broadcaster::new();
        let mut rx = b.subscribe();
        let lib = Uuid::now_v7();
        let ev = |created| ScanEvent::SeriesUpdated {
            library_id: lib,
            series_id: Uuid::now_v7(),
            name: "x".into(),
            created,
            cover_url: None,
        };
        assert!(b.series_updated_due(lib, false));
        b.emit(ev(false));
        assert!(!b.series_updated_due(lib, false));
        assert!(b.series_updated_due(lib, true));
        b.emit(ev(true));
        b.emit(ev(false)); // throttled
        let mut got = Vec::new();
        while let Ok(e) = rx.try_recv() {
            got.push(e);
        }
        assert_eq!(got.len(), 2);
    }
}
