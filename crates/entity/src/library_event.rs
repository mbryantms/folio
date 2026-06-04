//! Observability split M1 — durable library event log.
//!
//! One row per itemized library-subsystem fact: scan lifecycle, per-entity
//! changes (issue/series added/updated/removed), thumbnail + cover generation,
//! metadata application, archive rewrites, and errors. This is the canonical
//! "Library stream" record — durable and queryable, distinct from the ephemeral
//! ring buffer (server runtime) and `audit_log` (server compliance trail).
//!
//! `category` / `action` / `entity_type` are free-text discriminators (no DB
//! CHECK) so new event kinds never need a migration — mirroring
//! [`super::library_health_issue`]. Only `severity` is constrained
//! (`info` | `warning` | `error`).
//!
//! `batch_id` links to the `scan_batch` table (M5); it is a plain nullable
//! column until that migration adds the FK.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "library_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub library_id: Uuid,
    /// The scan that produced this event. `None` for events not tied to a
    /// scan run (e.g. an ad-hoc metadata apply). Detached (set null) when the
    /// scan run is pruned.
    #[sea_orm(nullable)]
    pub scan_run_id: Option<Uuid>,
    /// The scan-all batch this event's scan belongs to (M5). `None` for
    /// single-library scans and non-scan events.
    #[sea_orm(nullable)]
    pub batch_id: Option<Uuid>,
    /// Coarse domain bucket: `file` | `series` | `issue` | `cover` |
    /// `thumbnail` | `metadata` | `archive` | `health` | `scan`.
    pub category: String,
    /// The kind of entity the event is about (`issue`, `series`, `cover`, …).
    #[sea_orm(nullable)]
    pub entity_type: Option<String>,
    /// Heterogeneous entity id (issues are string-keyed, series/library are
    /// uuid) — stored as text, matching `scan_runs.issue_id`.
    #[sea_orm(nullable)]
    pub entity_id: Option<String>,
    /// Human-readable label for the entity (series name, issue title) so the
    /// UI doesn't need a join to render the row.
    #[sea_orm(nullable)]
    pub entity_label: Option<String>,
    /// What happened: `added` | `updated` | `removed` | `restored` |
    /// `converted` | `generated` | `applied` | `errored` | ….
    pub action: String,
    /// `info` | `warning` | `error`.
    pub severity: String,
    /// One-line human-readable summary of the event.
    pub summary: String,
    /// Typed event-specific body (counts, before/after, error detail).
    #[sea_orm(nullable)]
    pub detail: Option<Json>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id",
        on_delete = "Cascade"
    )]
    Library,
    #[sea_orm(
        belongs_to = "super::scan_run::Entity",
        from = "Column::ScanRunId",
        to = "super::scan_run::Column::Id",
        on_delete = "SetNull"
    )]
    ScanRun,
    #[sea_orm(
        belongs_to = "super::scan_batch::Entity",
        from = "Column::BatchId",
        to = "super::scan_batch::Column::Id",
        on_delete = "SetNull"
    )]
    ScanBatch,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}
impl Related<super::scan_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanRun.def()
    }
}
impl Related<super::scan_batch::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanBatch.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
