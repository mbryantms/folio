//! Observability split M2 — durable library-event writer surface.
//!
//! The canonical write path for the `library_events` table (M1). This is the
//! **Library stream**: a durable, itemized record of everything the library
//! subsystem does — scan lifecycle, per-entity changes, thumbnail/cover
//! generation, metadata application, archive rewrites, errors.
//!
//! It is deliberately **observational only**: writing an event never mutates
//! provider-touched data and must not be confused with the audited
//! [`crate::metadata::writers`] surface (which owns the actual data writes).
//! Logging an event here is the equivalent of `tracing::info!` — it records
//! that something happened, it does not make it happen.
//!
//! Two entry points, mirroring [`crate::audit`]:
//!   - [`record`] — a single event (fire-and-forget; errors are logged, never
//!     bubbled, because the underlying work has already succeeded).
//!   - [`record_many`] — bulk insert for a whole scan phase. The scanner can
//!     emit hundreds of rows per phase; one round-trip keeps that off the
//!     critical path (M3 wires the call sites).
//!
//! Vocabulary ([`Category`] / [`Action`] / [`Severity`]) is typed at the call
//! site for safety but stored as free text — adding a variant is a Rust-only
//! change, never a migration (only `severity` carries a DB CHECK). This
//! mirrors the JSON-blob philosophy of [`crate::library::health`].

use entity::library_event;
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Set, Statement,
};
use uuid::Uuid;

/// Coarse domain bucket for an event. Stored verbatim in
/// `library_events.category`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Scan lifecycle (started / completed / failed / cancelled).
    Scan,
    /// A file the scanner saw (added / skipped / malformed / converted).
    File,
    /// A series row created / updated / removed.
    Series,
    /// An issue row created / updated / removed / restored.
    Issue,
    /// Cover extraction or replacement.
    Cover,
    /// Thumbnail generation (cover strip / page map).
    Thumbnail,
    /// Metadata applied to a series/issue (provider or sidecar).
    Metadata,
    /// Archive rewrite (page edit, CBR→CBZ, sidecar writeback).
    Archive,
    /// A health issue surfaced during the scan.
    Health,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::File => "file",
            Self::Series => "series",
            Self::Issue => "issue",
            Self::Cover => "cover",
            Self::Thumbnail => "thumbnail",
            Self::Metadata => "metadata",
            Self::Archive => "archive",
            Self::Health => "health",
        }
    }
}

/// What happened to the subject. Stored verbatim in `library_events.action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Started,
    Completed,
    Cancelled,
    Added,
    Updated,
    Removed,
    Restored,
    Skipped,
    Converted,
    Generated,
    Applied,
    Errored,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Added => "added",
            Self::Updated => "updated",
            Self::Removed => "removed",
            Self::Restored => "restored",
            Self::Skipped => "skipped",
            Self::Converted => "converted",
            Self::Generated => "generated",
            Self::Applied => "applied",
            Self::Errored => "errored",
        }
    }
}

/// Severity bucket. The values here are the **only** ones the DB CHECK
/// constraint (`library_events_severity_chk`) admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A single event to be recorded. Build with [`NewEvent::new`] + the chained
/// setters; the required classifiers (library, category, action, severity,
/// summary) are constructor args so a call site can't forget them.
#[derive(Debug, Clone)]
pub struct NewEvent {
    library_id: Uuid,
    scan_run_id: Option<Uuid>,
    batch_id: Option<Uuid>,
    category: Category,
    entity_type: Option<&'static str>,
    entity_id: Option<String>,
    entity_label: Option<String>,
    action: Action,
    severity: Severity,
    summary: String,
    detail: Option<serde_json::Value>,
}

impl NewEvent {
    pub fn new(
        library_id: Uuid,
        category: Category,
        action: Action,
        severity: Severity,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            library_id,
            scan_run_id: None,
            batch_id: None,
            category,
            entity_type: None,
            entity_id: None,
            entity_label: None,
            action,
            severity,
            summary: summary.into(),
            detail: None,
        }
    }

    /// Link the event to the scan run that produced it.
    #[must_use]
    pub fn scan_run(mut self, id: Uuid) -> Self {
        self.scan_run_id = Some(id);
        self
    }

    /// Link the event to the scan-all batch (M5) its scan belongs to.
    #[must_use]
    pub fn batch(mut self, id: Uuid) -> Self {
        self.batch_id = Some(id);
        self
    }

    /// Attach the subject entity: its kind (`"issue"`, `"series"`, …), id, and
    /// an optional human label so the UI renders the row without a join.
    #[must_use]
    pub fn entity(
        mut self,
        entity_type: &'static str,
        id: impl Into<String>,
        label: Option<String>,
    ) -> Self {
        self.entity_type = Some(entity_type);
        self.entity_id = Some(id.into());
        self.entity_label = label;
        self
    }

    /// Attach a typed JSON body (counts, before/after, error detail).
    #[must_use]
    pub fn detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }

    fn into_active_model(self) -> library_event::ActiveModel {
        library_event::ActiveModel {
            id: Set(Uuid::now_v7()),
            library_id: Set(self.library_id),
            scan_run_id: Set(self.scan_run_id),
            batch_id: Set(self.batch_id),
            category: Set(self.category.as_str().to_owned()),
            entity_type: Set(self.entity_type.map(str::to_owned)),
            entity_id: Set(self.entity_id),
            entity_label: Set(self.entity_label),
            action: Set(self.action.as_str().to_owned()),
            severity: Set(self.severity.as_str().to_owned()),
            summary: Set(self.summary),
            detail: Set(self.detail),
            created_at: Set(chrono::Utc::now().fixed_offset()),
        }
    }
}

/// Record one library event. Fire-and-forget: a write failure is logged but
/// never bubbles, because the work the event describes has already happened.
pub async fn record(db: &DatabaseConnection, event: NewEvent) {
    let category = event.category;
    let action = event.action;
    if let Err(e) = event.into_active_model().insert(db).await {
        tracing::error!(
            error = %e,
            category = category.as_str(),
            action = action.as_str(),
            "library_event write failed",
        );
    }
}

/// Bulk-record a batch of events in a single round-trip — the perf path for a
/// scan phase. Empty input is a no-op. Fire-and-forget, like [`record`].
pub async fn record_many(db: &DatabaseConnection, events: Vec<NewEvent>) {
    if events.is_empty() {
        return;
    }
    let count = events.len();
    let models: Vec<library_event::ActiveModel> = events
        .into_iter()
        .map(NewEvent::into_active_model)
        .collect();
    // Chunk to stay well under Postgres's 65535 bind-parameter cap.
    // `library_event` has 13 columns, so a single `insert_many` tops out near
    // ~5040 rows — and a first scan of a large library emits ~1 event per added
    // issue, so an unchunked write would fail wholesale and silently drop the
    // entire scan manifest. 500 rows/statement (matching `HealthCollector`)
    // keeps each statement far inside the cap (OPS-1).
    for chunk in models.chunks(EVENT_INSERT_CHUNK) {
        if let Err(e) = library_event::Entity::insert_many(chunk.to_vec())
            .exec(db)
            .await
        {
            tracing::error!(error = %e, count, "library_event bulk write failed");
        }
    }
}

/// Rows per `insert_many` statement in [`record_many`] — bounded by the
/// Postgres bind-parameter cap (see the comment there). Matches the
/// `HealthCollector::finalize` chunk size.
const EVENT_INSERT_CHUNK: usize = 500;

/// Retention sweep for `library_events` (observability-split M4). A single
/// scan can write thousands of itemized rows, so the table is bounded two
/// ways at once: rows older than `retention_days`, **and** rows beyond the
/// most-recent `max_per_library` per library (a backstop for a library that
/// re-scans heavily inside the time window). Returns the number of rows
/// deleted. Called by the daily cron in [`crate::jobs::scheduler`].
///
/// Postgres-only — the workspace pins sea-orm to sqlx-postgres. Mirrors the
/// `ROW_NUMBER() OVER (PARTITION BY library_id …)` shape of
/// [`crate::api::scan_runs::prune`].
pub async fn prune(
    db: &DatabaseConnection,
    retention_days: i64,
    max_per_library: u64,
) -> anyhow::Result<u64> {
    let cutoff = chrono::Utc::now().fixed_offset() - chrono::Duration::days(retention_days);
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            DELETE FROM library_events
            WHERE id IN (
                SELECT id FROM (
                    SELECT id, created_at, ROW_NUMBER() OVER (
                        PARTITION BY library_id ORDER BY created_at DESC
                    ) AS rn
                    FROM library_events
                ) AS sub
                WHERE sub.rn > $1 OR sub.created_at < $2
            )
        "#,
        [(max_per_library as i64).into(), cutoff.into()],
    );
    let res = db.execute(stmt).await?;
    Ok(res.rows_affected())
}

/// Per-scan buffer of library events, drained once at scan completion via
/// [`EventCollector::flush`]. Mirrors the threading + merge shape of
/// [`crate::library::health::HealthCollector`]: each parallel folder worker
/// gets a local collector, the consume loop `merge`s them into the top-level
/// collector, and the finalize path flushes the whole batch in one
/// [`record_many`]. Keeping the DB write off the parallel workers avoids
/// connection-pool contention on the scan hot path.
///
/// The collector bakes in `library_id` + `scan_run_id` (+ optional `batch_id`)
/// so call sites only supply the event-specific bits via [`Self::build`].
pub struct EventCollector {
    library_id: Uuid,
    scan_run_id: Option<Uuid>,
    batch_id: Option<Uuid>,
    events: Vec<NewEvent>,
}

impl EventCollector {
    pub fn new(library_id: Uuid, scan_run_id: Option<Uuid>) -> Self {
        Self {
            library_id,
            scan_run_id,
            batch_id: None,
            events: Vec::new(),
        }
    }

    /// Stamp the scan-all batch (M5) every event inherits.
    #[must_use]
    pub fn with_batch(mut self, batch_id: Option<Uuid>) -> Self {
        self.batch_id = batch_id;
        self
    }

    /// Start an event pre-filled with this collector's library / scan / batch
    /// ids. Chain `.entity(...)` / `.detail(...)` then hand it to
    /// [`Self::push`]. Split from `push` (rather than a single
    /// build-and-push call) so the immutable `build` borrow ends before the
    /// mutable `push` borrow — no borrow-checker fight at the call site.
    pub fn build(
        &self,
        category: Category,
        action: Action,
        severity: Severity,
        summary: impl Into<String>,
    ) -> NewEvent {
        let mut e = NewEvent::new(self.library_id, category, action, severity, summary);
        if let Some(s) = self.scan_run_id {
            e = e.scan_run(s);
        }
        if let Some(b) = self.batch_id {
            e = e.batch(b);
        }
        e
    }

    /// Buffer a built event.
    pub fn push(&mut self, event: NewEvent) {
        self.events.push(event);
    }

    /// Convenience for the common no-entity / no-detail case.
    pub fn record(
        &mut self,
        category: Category,
        action: Action,
        severity: Severity,
        summary: impl Into<String>,
    ) {
        let e = self.build(category, action, severity, summary);
        self.push(e);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Fold a per-worker collector into this one. Ids are assumed identical
    /// (same scan); only the buffered events move.
    pub fn merge(&mut self, mut other: EventCollector) {
        self.events.append(&mut other.events);
    }

    /// Flush the whole buffer in one bulk insert. Consumes the collector.
    pub async fn flush(self, db: &DatabaseConnection) {
        record_many(db, self.events).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_strings_match_db_check() {
        // These three are the only values `library_events_severity_chk`
        // admits — a drift here would make every write fail at runtime.
        assert_eq!(Severity::Info.as_str(), "info");
        assert_eq!(Severity::Warning.as_str(), "warning");
        assert_eq!(Severity::Error.as_str(), "error");
    }

    #[test]
    fn builder_populates_active_model() {
        let lib = Uuid::now_v7();
        let scan = Uuid::now_v7();
        let am = NewEvent::new(
            lib,
            Category::Issue,
            Action::Added,
            Severity::Info,
            "Added issue #1",
        )
        .scan_run(scan)
        .entity("issue", "issue-1", Some("Saga #1".to_owned()))
        .detail(serde_json::json!({"page_count": 24}))
        .into_active_model();

        assert_eq!(am.library_id.as_ref(), &lib);
        assert_eq!(am.scan_run_id.as_ref(), &Some(scan));
        assert_eq!(am.category.as_ref(), "issue");
        assert_eq!(am.action.as_ref(), "added");
        assert_eq!(am.severity.as_ref(), "info");
        assert_eq!(am.entity_type.as_ref(), &Some("issue".to_owned()));
        assert_eq!(am.entity_id.as_ref(), &Some("issue-1".to_owned()));
        assert_eq!(am.entity_label.as_ref(), &Some("Saga #1".to_owned()));
        assert!(am.detail.as_ref().is_some());
    }

    #[test]
    fn collector_stamps_ids_and_merges() {
        let lib = Uuid::now_v7();
        let scan = Uuid::now_v7();
        let batch = Uuid::now_v7();
        let mut a = EventCollector::new(lib, Some(scan)).with_batch(Some(batch));
        a.record(Category::Scan, Action::Started, Severity::Info, "started");
        let e = a
            .build(Category::Issue, Action::Added, Severity::Info, "added")
            .entity("issue", "i1", None);
        a.push(e);

        let mut b = EventCollector::new(lib, Some(scan));
        b.record(Category::Series, Action::Updated, Severity::Info, "s");
        a.merge(b);
        assert_eq!(a.len(), 3);

        // The pre-filled ids land on every built event.
        let am = a
            .build(Category::Issue, Action::Removed, Severity::Warning, "gone")
            .into_active_model();
        assert_eq!(am.library_id.as_ref(), &lib);
        assert_eq!(am.scan_run_id.as_ref(), &Some(scan));
        assert_eq!(am.batch_id.as_ref(), &Some(batch));
    }
}
