use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One entry in the structural diff history for a CBL refresh run.
/// Append-only — populated by every refresh trigger (`'manual'` |
/// `'scheduled'` | `'post_scan'`). Drives the History tab and the
/// "what changed?" toast on next page load.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cbl_refresh_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub cbl_list_id: Uuid,
    pub ran_at: DateTimeWithTimeZone,
    pub trigger: String,
    /// `true` when the refresh fetched new bytes (catalog blob SHA
    /// changed, ETag missed, or SHA-256 differed). `false` when the
    /// refresh was a re-match against the existing bytes.
    pub upstream_changed: bool,
    #[sea_orm(nullable)]
    pub prev_blob_sha: Option<String>,
    #[sea_orm(nullable)]
    pub new_blob_sha: Option<String>,
    pub added_count: i32,
    pub removed_count: i32,
    pub reordered_count: i32,
    pub rematched_count: i32,
    /// `{ added: [{position, series, number}], removed: [...], reordered: [...] }`
    #[sea_orm(nullable)]
    pub diff_summary: Option<Json>,
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
}

impl Related<super::cbl_list::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CblList.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
