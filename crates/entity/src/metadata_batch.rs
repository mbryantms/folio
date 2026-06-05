//! Bulk-metadata batch grouping (refine-bulk-metadata M1).
//!
//! One row per bulk fetch ("fetch all issues in a series", "fetch a saved
//! view", a user-triggered library refresh). The per-series/per-issue
//! [`super::metadata_run`] rows it enqueues carry `batch_id` pointing here so
//! the admin Review queue aggregates live progress + a consolidated accept
//! surface. Mirrors [`super::scan_batch`].

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "metadata_batch")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// NULL for cross-library saved-view batches.
    #[sea_orm(nullable)]
    pub library_id: Option<Uuid>,
    /// `'series_issues' | 'saved_view' | 'library_refresh'`.
    pub scope: String,
    /// `orchestrator::trigger_kind` constant (today always `'manual'` — bulk
    /// fetch holds everything for review, never auto-applies).
    pub trigger_kind: String,
    /// `'running' | 'completed' | 'partial_failed' | 'awaiting_quota'`,
    /// derived from the member-run aggregate.
    pub status: String,
    /// Child runs enqueued at fan-out — the only stored count; everything
    /// else is a `GROUP BY batch_id` over the member runs.
    pub items_total: i32,
    /// Who triggered it; `None` for system flows. No FK (mirrors
    /// `audit_log.actor_id`).
    #[sea_orm(nullable)]
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub ended_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::metadata_run::Entity")]
    MetadataRun,
}

impl Related<super::metadata_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MetadataRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
