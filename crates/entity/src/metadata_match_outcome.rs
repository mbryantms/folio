//! Per-run outcome telemetry for the matching engine.
//!
//! Companion to `metadata_run` / `metadata_run_candidate`: one row per
//! completed search captures the **shape** of the ranked list (single
//! strong match? multiple plausible? no covers matched?) plus the
//! top + runner-up scores so the admin dashboard can render bucket
//! distribution trends over rolling windows.
//!
//! Matching-accuracy-1.0 M0. Pre-`metadata_run` CASCADE handles
//! deletes; the nightly prune sweeps anything older than 90 days.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "metadata_match_outcome")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub run_id: Uuid,
    /// `'series' | 'issue' | 'library' | 'bulk_refresh'` — same string
    /// space as `metadata_run.scope`. Denormalized so the dashboard
    /// can group without a join.
    pub scope: String,
    /// One of: `'single_good' | 'multi_good' | 'single_bad_cover' |
    /// 'multi_bad_cover' | 'no_match'`. See
    /// [`crate::metadata::match_outcome::MatchOutcomeKind`] for the
    /// canonical encoding.
    pub outcome_kind: String,
    pub top_score: f32,
    /// Top candidate's cover Hamming distance to the local cover.
    /// NULL when no phash was available on either side (most M0
    /// rows — matcher M4 will start stamping this).
    #[sea_orm(nullable)]
    pub top_hamming: Option<i32>,
    /// Runner-up score for the gap-to-next-best signal. NULL when
    /// the ranked list has 0 or 1 candidates.
    #[sea_orm(nullable)]
    pub second_score: Option<f32>,
    #[sea_orm(nullable)]
    pub second_hamming: Option<i32>,
    pub candidate_count: i32,
    pub created_at: DateTimeWithTimeZone,
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
