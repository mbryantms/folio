use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One entry in a user-owned collection (saved view with
/// `kind = 'collection'`). Each row references *exactly one* of series
/// or issue; the schema-level XOR check (`collection_entries_ref_xor_chk`)
/// guarantees the invariant. `position` is 0-indexed reading order and
/// unique within a collection (deferrable, so reorder can swap in a
/// single tx without sentinel juggling).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "collection_entries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub saved_view_id: Uuid,
    pub position: i32,
    /// `'series'` or `'issue'`. Caller picks; the XOR check rejects
    /// rows where the populated FK doesn't match.
    pub entry_kind: String,
    #[sea_orm(nullable)]
    pub series_id: Option<Uuid>,
    /// `issues.id` is `BLAKE3` hex (TEXT), matching the rest of the
    /// schema.
    #[sea_orm(nullable)]
    pub issue_id: Option<String>,
    pub added_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::saved_view::Entity",
        from = "Column::SavedViewId",
        to = "super::saved_view::Column::Id",
        on_delete = "Cascade"
    )]
    SavedView,
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
}

impl Related<super::saved_view::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SavedView.def()
    }
}
impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}
impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
