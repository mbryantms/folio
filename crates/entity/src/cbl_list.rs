use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One imported CBL reading list. Three source kinds: `'upload'` (raw
/// bytes posted by a user), `'url'` (HTTPS endpoint serving a `.cbl`),
/// `'catalog'` (resolved from a `catalog_sources` row + `catalog_path`).
/// CHECK constraint on the table enforces shape per kind.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cbl_lists")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// `None` for admin-curated system CBLs.
    #[sea_orm(nullable)]
    pub owner_user_id: Option<Uuid>,
    pub source_kind: String,
    /// Populated for `source_kind` of `'url'` and `'catalog'` (the raw
    /// blob URL when catalog-sourced).
    #[sea_orm(nullable)]
    pub source_url: Option<String>,
    /// FK to `catalog_sources.id` when `source_kind = 'catalog'`.
    #[sea_orm(nullable)]
    pub catalog_source_id: Option<Uuid>,
    /// Path within the catalog repo (e.g. `Image/Invincible Universe.cbl`).
    #[sea_orm(nullable)]
    pub catalog_path: Option<String>,
    /// Git blob SHA from the last successful catalog fetch. Used for
    /// change detection — same SHA → upstream unchanged, skip refetch.
    #[sea_orm(nullable)]
    pub github_blob_sha: Option<String>,
    /// HTTP ETag fallback for direct-URL imports.
    #[sea_orm(nullable)]
    pub source_etag: Option<String>,
    #[sea_orm(nullable)]
    pub source_last_modified: Option<String>,
    /// SHA-256 digest of the `.cbl` bytes — final-fallback change signal
    /// when neither ETag nor blob SHA are available.
    pub raw_sha256: Vec<u8>,
    /// Original `.cbl` XML preserved verbatim for re-parse after a
    /// matcher / schema change without needing to refetch.
    pub raw_xml: String,
    /// `<Name>` from the file.
    pub parsed_name: String,
    /// `true` when the parsed file shipped non-empty `<Matchers>` rules
    /// — informational badge in the UI; v1 does not evaluate matchers.
    pub parsed_matchers_present: bool,
    /// `<NumIssues>` from the file. Informational; trust the entry count
    /// for actual lookups.
    #[sea_orm(nullable)]
    pub num_issues_declared: Option<i32>,
    /// User/admin-authored description (markdown supported in the UI).
    #[sea_orm(nullable)]
    pub description: Option<String>,
    pub imported_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub last_refreshed_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub last_match_run_at: Option<DateTimeWithTimeZone>,
    /// Cron expression for scheduled refreshes; `None` = manual-only.
    #[sea_orm(nullable)]
    pub refresh_schedule: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::OwnerUserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
    #[sea_orm(
        belongs_to = "super::catalog_source::Entity",
        from = "Column::CatalogSourceId",
        to = "super::catalog_source::Column::Id",
        on_delete = "SetNull"
    )]
    CatalogSource,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::catalog_source::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CatalogSource.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
