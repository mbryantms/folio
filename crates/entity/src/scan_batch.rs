//! Observability split M5 — scan-all batch grouping.
//!
//! One row per "Scan all" action; the per-library [`super::scan_run`] rows it
//! enqueues carry `batch_id` pointing here. The admin Scan-all dashboard
//! aggregates live progress + a post-run roll-up across the batch's members.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "scan_batch")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Trigger discriminator — today always `scan_all`.
    pub kind: String,
    /// Who triggered it; `None` for system/cron runs. No FK (mirrors
    /// `audit_log.actor_id`).
    #[sea_orm(nullable)]
    pub actor_id: Option<Uuid>,
    /// Whether the batch was a force (content-verify) scan.
    pub force: bool,
    pub started_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub ended_at: Option<DateTimeWithTimeZone>,
    /// Number of libraries the batch enqueued.
    pub library_count: i32,
    /// `running` | `complete` | `partial_failed` | `failed`, derived as the
    /// member runs finish.
    pub state: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::scan_run::Entity")]
    ScanRun,
}

impl Related<super::scan_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
