use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "libraries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub name: String,
    /// URL-safe identifier, globally unique across libraries. Allocated at
    /// create time via `crate::slug::allocate_slug`; admins can rename.
    pub slug: String,
    #[sea_orm(unique)]
    pub root_path: String,
    pub default_language: String,
    pub default_reading_direction: String,
    pub dedupe_by_content: bool,
    #[sea_orm(nullable)]
    pub scan_schedule_cron: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub last_scan_at: Option<DateTimeWithTimeZone>,
    /// User-configured ignore globs (spec §5.2). JSON array of glob strings.
    pub ignore_globs: Json,
    /// When true, files without ComicInfo.xml emit info-level health issues
    /// (spec §10, §11 — default false).
    pub report_missing_comicinfo: bool,
    /// When false, the file-watch subsystem is suppressed regardless of mount
    /// detection (spec §3.1, §11).
    pub file_watch_enabled: bool,
    /// Days a soft-deleted issue/series stays in pending state before
    /// `removal_confirmed_at` is auto-set (spec §4.7, §14.1).
    pub soft_delete_days: i32,
    /// When false, the post-scan worker skips thumbnail enqueue for this
    /// library and the catchup sweep ignores it. Existing on-disk thumbnails
    /// keep serving.
    pub thumbnails_enabled: bool,
    /// Thumbnail encode format. One of `webp` | `jpeg` | `png`. Changing
    /// the value does not auto-regenerate; the admin force-recreate action
    /// applies the new format.
    pub thumbnail_format: String,
    /// Encoder quality for cover thumbnails, 0..=100. Lower values trade
    /// detail for smaller files on lossy formats.
    pub thumbnail_cover_quality: i32,
    /// Encoder quality for reader page-strip thumbnails, 0..=100.
    pub thumbnail_page_quality: i32,
    /// When true, the post-scan pipeline auto-enqueues page-strip thumbnails
    /// alongside the always-on cover thumbnails. Default false because page
    /// thumbs are pricier (one image per page, dozens per issue) — admins
    /// opt in at library-create time or via library settings. Manual
    /// generation via the admin "Generate missing pages" button always
    /// works regardless of this flag.
    #[serde(default)]
    pub generate_page_thumbs_on_scan: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::series::Entity")]
    Series,
    #[sea_orm(has_many = "super::issue::Entity")]
    Issue,
    #[sea_orm(has_many = "super::scan_run::Entity")]
    ScanRun,
    #[sea_orm(has_many = "super::library_health_issue::Entity")]
    LibraryHealthIssue,
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
impl Related<super::scan_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanRun.def()
    }
}
impl Related<super::library_health_issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::LibraryHealthIssue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
