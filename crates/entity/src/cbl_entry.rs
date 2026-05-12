use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One `<Book>` from an imported CBL. Position is 0-indexed reading
/// order, unique within a list. `match_status` classifies the resolution
/// outcome:
///
///   - `matched` — exactly one issue resolved (via CV ID, Metron ID, or
///     name+volume+number fallback); `match_method` and
///     `match_confidence` are populated.
///   - `ambiguous` — multiple candidates above the trigram threshold;
///     `ambiguous_candidates` carries the top picks for the Resolution
///     UI to surface.
///   - `missing` — no candidate found.
///   - `manual` — user-resolved override; preserved across automatic
///     re-runs unless explicitly cleared.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cbl_entries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub cbl_list_id: Uuid,
    pub position: i32,
    pub series_name: String,
    pub issue_number: String,
    #[sea_orm(nullable)]
    pub volume: Option<String>,
    #[sea_orm(nullable)]
    pub year: Option<String>,
    #[sea_orm(nullable)]
    pub cv_series_id: Option<i32>,
    #[sea_orm(nullable)]
    pub cv_issue_id: Option<i32>,
    #[sea_orm(nullable)]
    pub metron_series_id: Option<i32>,
    #[sea_orm(nullable)]
    pub metron_issue_id: Option<i32>,
    /// FK to `issues.id` (BLAKE3 hex). NULL when `match_status != 'matched' / 'manual'`.
    #[sea_orm(nullable)]
    pub matched_issue_id: Option<String>,
    pub match_status: String,
    #[sea_orm(nullable)]
    pub match_method: Option<String>,
    #[sea_orm(nullable)]
    pub match_confidence: Option<f32>,
    /// Top candidates from the trigram-fallback path when `match_status =
    /// 'ambiguous'`. Shape: `[{ issue_id, series_name, similarity }, …]`.
    #[sea_orm(nullable)]
    pub ambiguous_candidates: Option<Json>,
    #[sea_orm(nullable)]
    pub user_resolved_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub matched_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cbl_list::Entity",
        from = "Column::CblListId",
        to = "super::cbl_list::Column::Id",
        on_delete = "Cascade"
    )]
    CblList,
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::MatchedIssueId",
        to = "super::issue::Column::Id",
        on_delete = "SetNull"
    )]
    Issue,
}

impl Related<super::cbl_list::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CblList.def()
    }
}
impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
