//! A page Folio kept from a damaged archive. See
//! `m20271127_000002_issue_damaged_pages` and
//! `server::library::damaged_pages`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_damaged_pages")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    /// The page's entry name in the CBZ.
    #[sea_orm(primary_key, auto_increment = false)]
    pub entry_name: String,
    /// The entry's name in the original archive.
    pub source_entry: String,
    /// 0-based page index as of the last scan that read the file.
    pub page_index: i32,
    /// `salvaged` (the bytes UnRAR decoded) or `placeholder` (nothing
    /// usable survived; Folio wrote a stand-in page).
    pub kind: String,
    /// Static reason from `archive::damage`.
    pub reason: String,
    pub declared_size: i64,
    pub recovered_size: i64,
    /// CRC-32 of the bytes the CBZ holds for this entry.
    pub entry_crc32: i64,
    /// File name of the archive the page came from.
    pub source_name: String,
    /// Full path of that archive's backup, kept past retention.
    pub source_backup: Option<String>,
    pub detected_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
