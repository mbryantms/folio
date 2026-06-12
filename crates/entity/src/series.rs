use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Note: `search_doc` is a Postgres GENERATED ALWAYS column (tsvector,
// see m20260301_000001_search_docs) and is intentionally omitted from
// this entity. Sea-ORM has no first-class read-only column support;
// including it would require custom ActiveModel plumbing that rejects
// writes. The column is documented as an entity-vs-DB parity
// exception in docs/dev/schema-evolution.md and allow-listed by the
// schema_parity regression test.

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub library_id: Uuid,
    pub name: String,
    pub normalized_name: String,
    /// URL-safe identifier, globally unique across all series. Allocated at
    /// scanner-insert time via `crate::slug::allocate_slug` with the
    /// series' year/volume/publisher as fallback disambiguators. Distinct
    /// from `normalized_name`, which is for dedupe/CBL matching.
    pub slug: String,
    #[sea_orm(nullable)]
    pub year: Option<i32>,
    #[sea_orm(nullable)]
    pub volume: Option<i32>,
    #[sea_orm(nullable)]
    pub publisher: Option<String>,
    #[sea_orm(nullable)]
    pub imprint: Option<String>,
    pub status: String,
    #[sea_orm(nullable)]
    pub total_issues: Option<i32>,
    #[sea_orm(nullable)]
    pub age_rating: Option<String>,
    #[sea_orm(nullable)]
    pub summary: Option<String>,
    pub language_code: String,
    #[sea_orm(nullable)]
    pub series_group: Option<String>,
    pub alternate_names: Json,
    /// Sort name (drops leading articles, "The X-Men" → "X-Men, The").
    /// Metron exposes this natively; CV doesn't, so we'd derive at
    /// fetch time.
    #[sea_orm(nullable)]
    pub sort_name: Option<String>,
    /// Year the series ended. NULL for ongoing. Distinct from
    /// `status`: a series can be `ongoing` with `year_end IS NULL`,
    /// or `cancelled`/`completed` with `year_end IS NOT NULL`.
    #[sea_orm(nullable)]
    pub year_end: Option<i32>,
    /// `'ongoing' | 'limited' | 'one-shot' | 'annual' | 'hardcover'
    /// | 'tpb' | 'digital_chapter' | 'single_issue'`. Metron exposes
    /// natively; CV doesn't.
    #[sea_orm(nullable)]
    pub series_type: Option<String>,
    /// Alternate titles. JSON array of strings.
    pub aliases: Json,
    /// Short summary (1-2 sentences). Distinct from `summary` (long).
    #[sea_orm(nullable)]
    pub deck: Option<String>,
    /// FK to the canonical `publisher` entity. NULL until a fetch or
    /// scanner populates it. The legacy `publisher` TEXT column
    /// stays as the denormalized read-cache (matches the
    /// CSV-on-issues pattern).
    #[sea_orm(nullable)]
    pub publisher_id: Option<Uuid>,
    /// FK to the canonical `imprint` entity. NULL until populated.
    #[sea_orm(nullable)]
    pub imprint_id: Option<Uuid>,
    /// Last time an M4 Apply job touched this series from any
    /// provider. NULL = never synced.
    #[sea_orm(nullable)]
    pub last_metadata_sync_at: Option<DateTimeWithTimeZone>,
    /// When true, the weekly refresh cron + bulk-refresh fan-out
    /// skip this series. User-facing toggle on the series page.
    pub metadata_sync_paused: bool,
    /// Whether a Mylar3 `series.json` was present in the series folder at the
    /// last scan. `None` = scanned before this column existed (rescan to
    /// backfill); `Some(false)` = definitely absent.
    #[sea_orm(nullable)]
    pub series_json_present: Option<bool>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// Disk path of the series folder. Used as the fast-path identity match
    /// (spec §7.1.1). Nullable for legacy rows that predate scanner v1; the
    /// next scan backfills it.
    #[sea_orm(nullable)]
    pub folder_path: Option<String>,
    /// Last successful scan of *this series* (spec §4.4). Used to skip
    /// folders whose recursive mtime hasn't moved.
    #[sea_orm(nullable)]
    pub last_scanned_at: Option<DateTimeWithTimeZone>,
    /// Admin-set sticky match key (spec §7.4). When non-null, identity
    /// resolution returns `ExistingByMatchKey` and never overwrites.
    #[sea_orm(nullable)]
    pub match_key: Option<String>,
    /// Set by reconciliation when all issues in this series are removed
    /// (spec §4.7).
    #[sea_orm(nullable)]
    pub removed_at: Option<DateTimeWithTimeZone>,
    /// Set by the reconcile sweep job after `library.soft_delete_days`
    /// have elapsed since `removed_at` (spec §4.7).
    #[sea_orm(nullable)]
    pub removal_confirmed_at: Option<DateTimeWithTimeZone>,
    /// Sticky-override timestamp for `status`. Set by `PATCH /series/{slug}`
    /// whenever the body includes a `status` field. The post-scan
    /// `reconcile_series_status` helper checks this and skips the
    /// status write when non-null, so user-set values like `"hiatus"`
    /// or `"cancelled"` survive re-scans. The `total_issues` refresh
    /// is independent — manual override only freezes the status, not
    /// the count, so the Complete/Incomplete UI stays accurate.
    #[sea_orm(nullable)]
    pub status_user_set_at: Option<DateTimeWithTimeZone>,
    /// Per-series reading-direction override
    /// (`manga-and-bulk-metadata-1.0` M2). `"ltr"` / `"rtl"` / `"ttb"`
    /// or `NULL` meaning "Auto — inherit from user pref / library
    /// default at read time". ComicInfo `<Manga>YesAndRightToLeft</Manga>`
    /// on the issue still wins above this layer. M3's scanner heuristic
    /// auto-pins this to `"rtl"` when ≥80% of a series's issues carry
    /// the manga flag and the column is currently NULL; admin-set
    /// values are sticky and never overwritten.
    #[sea_orm(nullable)]
    pub reading_direction: Option<String>,
    /// Per-series OCR language override (OCR rework 1.0).
    /// `"western"` / `"manga"` or `NULL` meaning "Auto — infer from
    /// `reading_direction` (`rtl` ⇒ manga) at OCR time". A per-request
    /// `lang` override on `POST /me/issues/{id}/ocr` still wins above
    /// this layer.
    #[sea_orm(nullable)]
    pub text_language: Option<String>,
    /// When true, `/opds/v1/series/{id}` and `/opds/v2/series/{id}`
    /// render in strict `sort_number ASC` order regardless of the
    /// caller's progress. Default false — feeds auto-reorder the
    /// up-next issue to position 0 for users mid-series. Curators
    /// who want the first-issue-always-first UI flip this on per
    /// series. M2 of `opds-sync-cleanup-1.0`.
    pub preserve_canonical_order: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id"
    )]
    Library,
    #[sea_orm(has_many = "super::issue::Entity")]
    Issue,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}
impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

/// Punctuation-stripped, lowercased, whitespace-collapsed name used for
/// dedupe and CBL matching (§5.8). Stable across rescans.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_was_space = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_' || ch == '.')
            && !last_was_space
            && !out.is_empty()
        {
            out.push(' ');
            last_was_space = true;
        }
        // Other punctuation is stripped.
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punct_and_lowercases() {
        assert_eq!(normalize_name("Spider-Man (2018)"), "spider man 2018");
        assert_eq!(normalize_name("X-Men: Blue"), "x men blue");
        assert_eq!(normalize_name("  Saga  "), "saga");
        assert_eq!(normalize_name("Pokémon Adventures"), "pokémon adventures");
    }
}
