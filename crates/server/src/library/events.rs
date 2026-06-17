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
//! per-library to ≤10/sec by gating with a `last_emitted` timestamp map.
//! Other event types pass through immediately.
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
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_ms: Option<u64>,
    },
    #[serde(rename = "scan.series_updated")]
    SeriesUpdated {
        library_id: Uuid,
        series_id: Uuid,
        name: String,
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
    /// The job finished successfully. `pages` is the number of strip thumbs
    /// generated (cover is implied as +1).
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
            | Self::MetadataApplied { library_id, .. } => *library_id,
            // Library-agnostic; only reached via the SeriesUpdated-guarded
            // throttle, so this arm is never hit at runtime.
            Self::BackfillCompleted { .. } => Uuid::nil(),
        }
    }
}

#[derive(Clone)]
pub struct Broadcaster {
    sender: broadcast::Sender<ScanEvent>,
    series_updated_last: Arc<Mutex<HashMap<Uuid, Instant>>>,
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

    pub fn emit(&self, evt: ScanEvent) {
        // Throttle: only `SeriesUpdated` is rate-limited per-library.
        if matches!(&evt, ScanEvent::SeriesUpdated { .. }) {
            let library_id = evt.library_id();
            let now = Instant::now();
            let mut map = self.series_updated_last.lock().expect("throttle mutex");
            if let Some(prev) = map.get(&library_id)
                && now.duration_since(*prev) < SERIES_UPDATED_THROTTLE
            {
                return; // Skip — within throttle window.
            }
            map.insert(library_id, now);
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
