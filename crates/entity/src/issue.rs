use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Note: `search_doc` is a Postgres GENERATED ALWAYS column (tsvector,
// see m20260301_000001_search_docs) and is intentionally omitted from
// this entity. Sea-ORM has no first-class read-only column support;
// including it would require custom ActiveModel plumbing that rejects
// writes. The column is documented as an entity-vs-DB parity
// exception in docs/dev/schema-evolution.md and allow-listed by the
// schema_parity regression test.

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "issues")]
pub struct Model {
    /// BLAKE3 hex of either path or content (§5.1.2).
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub library_id: Uuid,
    pub series_id: Uuid,
    /// URL-safe identifier, unique within the parent series (`UNIQUE(series_id, slug)`).
    /// Allocated at scanner-insert time via `crate::slug::allocate_slug`,
    /// based on `number_raw` → `title` → BLAKE3 prefix.
    pub slug: String,
    #[sea_orm(unique)]
    pub file_path: String,
    pub file_size: i64,
    pub file_mtime: DateTimeWithTimeZone,
    pub state: String,
    pub content_hash: String,

    #[sea_orm(nullable)]
    pub title: Option<String>,
    #[sea_orm(nullable)]
    pub sort_number: Option<f64>,
    #[sea_orm(nullable)]
    pub number_raw: Option<String>,
    #[sea_orm(nullable)]
    pub volume: Option<i32>,
    #[sea_orm(nullable)]
    pub year: Option<i32>,
    #[sea_orm(nullable)]
    pub month: Option<i32>,
    #[sea_orm(nullable)]
    pub day: Option<i32>,
    #[sea_orm(nullable)]
    pub summary: Option<String>,
    #[sea_orm(nullable)]
    pub notes: Option<String>,
    #[sea_orm(nullable)]
    pub language_code: Option<String>,
    #[sea_orm(nullable)]
    pub format: Option<String>,
    #[sea_orm(nullable)]
    pub black_and_white: Option<bool>,
    #[sea_orm(nullable)]
    pub manga: Option<String>,
    #[sea_orm(nullable)]
    pub age_rating: Option<String>,
    #[sea_orm(nullable)]
    pub page_count: Option<i32>,
    pub pages: Json,
    pub comic_info_raw: Json,

    #[sea_orm(nullable)]
    pub alternate_series: Option<String>,
    #[sea_orm(nullable)]
    pub story_arc: Option<String>,
    #[sea_orm(nullable)]
    pub story_arc_number: Option<String>,
    #[sea_orm(nullable)]
    pub characters: Option<String>,
    #[sea_orm(nullable)]
    pub teams: Option<String>,
    #[sea_orm(nullable)]
    pub locations: Option<String>,
    #[sea_orm(nullable)]
    pub tags: Option<String>,
    #[sea_orm(nullable)]
    pub genre: Option<String>,
    #[sea_orm(nullable)]
    pub writer: Option<String>,
    #[sea_orm(nullable)]
    pub penciller: Option<String>,
    #[sea_orm(nullable)]
    pub inker: Option<String>,
    #[sea_orm(nullable)]
    pub colorist: Option<String>,
    #[sea_orm(nullable)]
    pub letterer: Option<String>,
    #[sea_orm(nullable)]
    pub cover_artist: Option<String>,
    #[sea_orm(nullable)]
    pub editor: Option<String>,
    #[sea_orm(nullable)]
    pub translator: Option<String>,
    #[sea_orm(nullable)]
    pub publisher: Option<String>,
    #[sea_orm(nullable)]
    pub imprint: Option<String>,
    #[sea_orm(nullable)]
    pub scan_information: Option<String>,
    #[sea_orm(nullable)]
    pub community_rating: Option<f64>,
    #[sea_orm(nullable)]
    pub review: Option<String>,
    #[sea_orm(nullable)]
    pub web_url: Option<String>,
    /// Short summary (1-2 sentences). Populated by M4 Apply jobs from
    /// ComicVine's `deck` field; distinct from `summary` (the long
    /// description). Free for the bulk-edit dialog + Apply jobs to
    /// set; not populated by the scanner today.
    #[sea_orm(nullable)]
    pub deck: Option<String>,
    /// Date the issue was first available in stores (Metron `store_date`,
    /// CV `store_date`). Distinct from the `year`/`month`/`day` trio
    /// which captures the cover date.
    #[sea_orm(nullable)]
    pub store_date: Option<Date>,
    /// Metron-specific final order cutoff date — when retailers had
    /// to finalize their orders. Useful future pullist features.
    #[sea_orm(nullable)]
    pub foc_date: Option<Date>,
    /// Catalog price as a display number (no FX, single-currency
    /// assumption). DOUBLE PRECISION rather than NUMERIC because this
    /// is display data, not financial math (matches `community_rating`).
    #[sea_orm(nullable)]
    pub price: Option<f64>,
    #[sea_orm(nullable)]
    pub sku: Option<String>,
    /// ComicVine staff review score (1-5 or NULL).
    #[sea_orm(nullable)]
    pub staff_rating: Option<f64>,
    /// Alternate titles / aliases the providers expose. JSON array of
    /// strings.
    pub aliases: Json,
    /// Last time an M4 Apply job (or a manual fetch from M5) touched
    /// this issue from any provider. NULL = never synced.
    #[sea_orm(nullable)]
    pub last_metadata_sync_at: Option<DateTimeWithTimeZone>,

    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,

    /// Reconciliation soft-delete (spec §4.7). Set when the file is no longer
    /// on disk; cleared if the file reappears.
    #[sea_orm(nullable)]
    pub removed_at: Option<DateTimeWithTimeZone>,
    /// Set by the reconcile sweep job after `library.soft_delete_days` have
    /// elapsed since `removed_at` (spec §4.7).
    #[sea_orm(nullable)]
    pub removal_confirmed_at: Option<DateTimeWithTimeZone>,
    /// Set when a file is modified in place (different content hash, same path)
    /// (spec §6.2). Points to the new issue id.
    #[sea_orm(nullable)]
    pub superseded_by: Option<String>,
    /// `Special` | `Annual` | `OneShot` | `TPB` | NULL (spec §6.5).
    #[sea_orm(nullable)]
    pub special_type: Option<String>,
    /// Hash algorithm version (spec §14.2). 1 = BLAKE3.
    pub hash_algorithm: i16,

    /// Thumbnail pipeline (M1): set when the post-scan thumbs worker has
    /// finished generating the cover thumbnail for this issue. Per-page strip
    /// thumbs are generated lazily for the reader page map.
    /// `None` means "still pending" — selected by the partial index
    /// `issues_thumbs_pending_idx`.
    #[sea_orm(nullable)]
    pub thumbnails_generated_at: Option<DateTimeWithTimeZone>,

    /// Code-side `THUMBNAIL_VERSION` at the time of generation. When the
    /// constant bumps (filter / quality / size change), the catchup sweep
    /// finds rows with `thumbnail_version < CURRENT` and re-enqueues.
    pub thumbnail_version: i32,

    /// Last failure reason from a thumb-gen attempt, if any. Cleared on the
    /// next success. Surfaced in the admin "errored thumbnails" view.
    #[sea_orm(nullable)]
    pub thumbnails_error: Option<String>,

    /// User-curated extra links (e.g. "ComicVine", "Wiki"). JSON array of
    /// `{label?: string, url: string}`. Distinct from `web_url` (which the
    /// scanner mirrors from ComicInfo's `Web` field) so a rescan can refresh
    /// `web_url` without dropping user-added links.
    pub additional_links: Json,

    /// Column names the user has overridden via `PATCH /issues/{id}`. The
    /// scanner consults this list on update and skips matching fields, so
    /// user edits are sticky across rescans (same pattern as `series.match_key`).
    pub user_edited: Json,

    /// ComicInfo `<Count>` from this issue's metadata — "the publisher
    /// claims this series has N issues total". Set per-issue (not just
    /// at series creation) so the scanner's reconciliation step can
    /// MAX-reduce a per-series total without re-parsing archives.
    /// Treat NULL or `<= 0` as "no signal".
    #[sea_orm(nullable)]
    pub comicinfo_count: Option<i32>,

    /// Timestamp of the last rewrite of this archive's bytes — set when
    /// either the sidecar-writeback path (`metadata-sidecar-writeback-1.0`)
    /// or the page-edit path (`archive-rewrite-1.0`) commits a new file
    /// via the atomic-swap helper. NULL means the archive has never been
    /// rewritten by Folio.
    #[sea_orm(nullable)]
    pub last_rewrite_at: Option<DateTimeWithTimeZone>,
    /// What kind of rewrite produced [`Self::last_rewrite_at`]:
    /// `'sidecar'` (metadata-only XML refresh) or `'edit'` (page bytes
    /// modified). NULL when `last_rewrite_at` is NULL. Helps the
    /// audit-log surface and the issue-detail status row tell them apart.
    #[sea_orm(nullable)]
    pub last_rewrite_kind: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id"
    )]
    Library,
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id"
    )]
    Series,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}
impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
