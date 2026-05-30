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
    /// Hard prerequisite for any code path that mutates archive bytes:
    /// sidecar writeback (`metadata-sidecar-writeback-1.0`) and page edits
    /// (`archive-rewrite-1.0`). Default false so no library starts
    /// rewriting bytes without explicit operator consent.
    #[serde(default)]
    pub allow_archive_writeback: bool,
    /// When true *and* `allow_archive_writeback` is also true, provider
    /// apply (`metadata-sidecar-writeback-1.0` M3+) writes fresh
    /// ComicInfo.xml + MetronInfo.xml into the archive and enqueues a
    /// scoped rescan. When false, apply takes the legacy DB-direct path.
    /// Per-library so operators migrate gradually.
    #[serde(default)]
    pub metadata_writeback_enabled: bool,
    /// How many `.bak` siblings to keep per archive (1..=5). Default 1
    /// — one undo slot. Capped at 5 to bound disk pressure.
    #[serde(default = "default_archive_backup_retain_count")]
    pub archive_backup_retain_count: i32,
    /// Auto-prune `.bak` files older than this. Default 30 days; `0` =
    /// keep forever.
    #[serde(default = "default_archive_backup_retain_days")]
    pub archive_backup_retain_days: i32,
    /// Encoder quality (60..=100) used when the page editor re-encodes a
    /// rotated / replaced JPEG page (`archive-rewrite-1.0`). Default 92.
    /// PNG / WebP pages stay lossless and ignore this.
    #[serde(default = "default_archive_writeback_jpeg_quality")]
    pub archive_writeback_jpeg_quality: i32,
    /// First-time CBR→CBZ conversion confirm gate. NULL until the
    /// operator acknowledges the conversion once for this library; set
    /// thereafter so subsequent CBR edits don't re-prompt
    /// (`archive-rewrite-1.0` M6).
    #[serde(default)]
    #[sea_orm(nullable)]
    pub cbr_convert_confirmed_at: Option<DateTimeWithTimeZone>,
    /// When true *and* `allow_archive_writeback` is also true, the scanner
    /// converts each `.cbr` it finds into a sibling `.cbz` in place (keeping
    /// the original as `.cbr.bak`) and then ingests the `.cbz` normally.
    /// When false (default), CBRs are skipped with an
    /// `UnsupportedArchiveFormat` health issue. Reuses the CBR→CBZ machinery
    /// from the page editor (`archive-rewrite-1.0` M6).
    #[serde(default)]
    pub auto_convert_cbr_on_scan: bool,
    /// Publisher names the matcher's pre-filter should drop before
    /// scoring. Comparison is case-insensitive against the
    /// title-sanitized form so "DC Comics" / "dc comics" / "DC" all
    /// match the same entry. Matching-accuracy-1.0 M3.
    #[serde(default)]
    pub metadata_publisher_blacklist: Json,
    /// When true, the filename inferer drops any leading numeric
    /// token before parsing the series name. Closes the common
    /// Mylar-style numbering case (`001 - Saga.cbz`).
    /// Matching-accuracy-1.0 M7.
    #[serde(default)]
    pub filename_ignore_leading_numbers: bool,
    /// When true, the filename inferer assumes issue `1` when no
    /// issue number is detected. Closes the one-shot / first-issue
    /// case where the operator's curation strips the `#1`.
    /// Matching-accuracy-1.0 M7.
    #[serde(default)]
    pub filename_assume_issue_one: bool,
    /// When true, non-manual searches (weekly cron, bulk-fetch
    /// toolbar) that produce a `MatchOutcomeKind::SingleGood`
    /// outcome auto-apply the top candidate. The user-edit
    /// precedence rule still fires, so pinned fields stay sacred.
    /// Manual searches (dialog kick) never auto-apply regardless.
    /// Matching-accuracy-1.0 M12.
    #[serde(default)]
    pub metadata_auto_apply_strong_matches: bool,
}

fn default_archive_backup_retain_count() -> i32 {
    1
}

fn default_archive_backup_retain_days() -> i32 {
    30
}

fn default_archive_writeback_jpeg_quality() -> i32 {
    92
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
