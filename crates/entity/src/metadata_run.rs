//! Run history for metadata search/apply jobs. Per-item detail
//! lives on `audit_log` rows linked via `payload->>'run_id'`; this
//! table exists for fast filter on run-level fields (status / scope
//! / library_id) that audit_log doesn't index well.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "metadata_run")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// `'series' | 'issue' | 'library' | 'bulk_refresh'`.
    pub scope: String,
    /// Entity id for `scope = 'series' | 'issue'`; NULL for library-
    /// wide or bulk runs. TEXT to natively hold both UUID and
    /// BLAKE3-hex issue ids.
    #[sea_orm(nullable)]
    pub scope_entity_id: Option<String>,
    #[sea_orm(nullable)]
    pub library_id: Option<Uuid>,
    #[sea_orm(nullable)]
    pub triggered_by: Option<Uuid>,
    /// `'manual' | 'weekly_refresh' | 'scanner' | 'bulk_action'`.
    pub trigger_kind: String,
    /// Providers attempted, in priority order at trigger time.
    pub providers: Vec<String>,
    /// `'queued' | 'searching' | 'applying' | 'completed' |
    /// 'awaiting_quota' | 'failed' | 'cancelled'`.
    pub status: String,
    pub started_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub finished_at: Option<DateTimeWithTimeZone>,
    pub items_total: i32,
    pub items_matched_high: i32,
    pub items_matched_medium: i32,
    pub items_matched_low: i32,
    pub items_no_match: i32,
    pub items_applied: i32,
    pub items_skipped: i32,
    pub items_failed: i32,
    #[sea_orm(nullable)]
    pub error_summary: Option<String>,
    /// When the run is paused on `awaiting_quota`, the time it
    /// should resume. NULL otherwise.
    #[sea_orm(nullable)]
    pub resume_after: Option<DateTimeWithTimeZone>,
    /// Serialized `SeriesQueryFacts` / `IssueQueryFacts` so the
    /// polling endpoint can render the search-in-flight UI without
    /// re-resolving the local entity. Nullable for legacy/null rows
    /// inserted before M3 (none exist on first deploy but the guard
    /// keeps Apply jobs (M4) usable without a query).
    #[sea_orm(nullable)]
    pub query: Option<Json>,
    /// Groups this run under a bulk-fetch [`super::metadata_batch`]. NULL for
    /// standalone per-entity runs (the common case). FK SET NULL on delete.
    #[sea_orm(nullable)]
    pub batch_id: Option<Uuid>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id",
        on_delete = "SetNull"
    )]
    Library,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::TriggeredBy",
        to = "super::user::Column::Id",
        on_delete = "SetNull"
    )]
    TriggeredBy,
    #[sea_orm(
        belongs_to = "super::metadata_batch::Entity",
        from = "Column::BatchId",
        to = "super::metadata_batch::Column::Id",
        on_delete = "SetNull"
    )]
    Batch,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}

impl Related<super::metadata_batch::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Batch.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
