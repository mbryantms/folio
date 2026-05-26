//! One ranked candidate emitted by a metadata search run.
//!
//! Lifecycle:
//! - inserted by the orchestrator at search completion
//! - read by the Candidates API + the Review Queue tab (M6)
//! - mutated by M4 Apply jobs (`applied_at`) and explicit user dismiss
//!   (`dismissed_at`)
//!
//! `score_breakdown` carries the per-component `Score` from
//! `metadata::matcher::Score` (name / year / publisher / issue_number /
//! volume) so the UI can render "why this score" tooltips.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "metadata_run_candidate")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: Uuid,
    /// 0-based rank from the orchestrator (lower = higher score).
    #[sea_orm(primary_key, auto_increment = false)]
    pub ordinal: i32,
    /// `'comicvine' | 'metron' | …` — matches `Source::as_str()`.
    pub source: String,
    pub external_id: String,
    /// `'high' | 'medium' | 'low'` — `Confidence::as_str()`.
    pub bucket: String,
    pub score: f32,
    pub score_breakdown: Json,
    /// Serialized `SeriesCandidate` or `IssueCandidate` from the
    /// provider response. M4 Apply jobs read this back when the user
    /// picks a candidate without re-fetching from the provider.
    pub candidate: Json,
    #[sea_orm(nullable)]
    pub applied_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub dismissed_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::metadata_run::Entity",
        from = "Column::RunId",
        to = "super::metadata_run::Column::Id",
        on_delete = "Cascade"
    )]
    Run,
}

impl Related<super::metadata_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
