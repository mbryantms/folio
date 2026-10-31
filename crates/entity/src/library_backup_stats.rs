//! Stored `.bak` footprint per library (audit 2026-11, N-L4). See
//! `m20271031_000001_library_backup_stats` for the lifecycle and
//! `server::library::backup_stats` for the writer.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "library_backup_stats")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub library_id: Uuid,
    pub file_count: i64,
    pub total_bytes: i64,
    #[sea_orm(nullable)]
    pub oldest_modified_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub newest_modified_at: Option<DateTimeWithTimeZone>,
    /// When the tally was last computed from the filesystem.
    pub computed_at: DateTimeWithTimeZone,
    /// Set by a catalogue-changing scan; cleared by the next recount.
    pub stale: bool,
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
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
