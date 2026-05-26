//! Per-issue cover storage. Primary + variants + back covers, per-row
//! provenance, per-cover perceptual hash (populated by M9). The cover
//! served as the issue's display thumbnail is `(kind='primary',
//! ordinal=0, is_active=true)`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issue_cover")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub issue_id: String,
    /// `'primary' | 'variant' | 'back' | 'incentive'`.
    pub kind: String,
    pub ordinal: i32,
    /// `'comicvine' | 'metron' | 'archive_extracted' | 'user_upload'`
    /// or NULL for legacy backfill rows.
    #[sea_orm(nullable)]
    pub source_provider: Option<String>,
    #[sea_orm(nullable)]
    pub source_external_id: Option<String>,
    #[sea_orm(nullable)]
    pub source_url: Option<String>,
    #[sea_orm(nullable)]
    pub variant_label: Option<String>,
    #[sea_orm(nullable)]
    pub variant_artist_person_id: Option<Uuid>,
    /// Relative to `{data_path}`. The cover-serving code prepends the
    /// configured data root.
    pub local_path: String,
    #[sea_orm(nullable)]
    pub width: Option<i32>,
    #[sea_orm(nullable)]
    pub height: Option<i32>,
    #[sea_orm(nullable)]
    pub phash: Option<i64>,
    #[sea_orm(nullable)]
    pub dhash: Option<i64>,
    #[sea_orm(nullable)]
    pub ahash: Option<i64>,
    pub fetched_at: DateTimeWithTimeZone,
    pub is_active: bool,
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
    #[sea_orm(
        belongs_to = "super::person::Entity",
        from = "Column::VariantArtistPersonId",
        to = "super::person::Column::Id",
        on_delete = "SetNull"
    )]
    VariantArtist,
}

impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
