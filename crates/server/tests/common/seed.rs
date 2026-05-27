//! Shared DB-fixture builders for integration tests.
//!
//! Before this module, every integration-test file (~20 of them)
//! hand-redeclared its own `seed_issue` / `seed_series` /
//! `seed_library` helper — 80 lines of `ActiveModel { ... }`
//! boilerplate each. Signatures diverged across files, so changing the
//! issue schema meant touching every fixture; the audit captured the
//! pattern as the highest-leverage cleanup in the test suite.
//!
//! Shape: each entity has a builder struct (`IssueSeed`, `SeriesSeed`,
//! `LibrarySeed`, `CblListSeed`, `CollectionSeed`) with sensible
//! defaults; call sites that want overrides use the `.with_*` chain.
//! For the most common shape, a free convenience function with the
//! exact signature the OPDS test cluster uses is also exported so the
//! mechanical migration is a one-line `use` swap.
//!
//! All builders construct rows with the same defaults the migrated
//! tests previously hand-rolled. New columns added later need to be
//! given a default here once, instead of in 20 places.

use chrono::Utc;
use entity::{
    cbl_entry::ActiveModel as CblEntryAM,
    cbl_list::ActiveModel as CblListAM,
    collection_entry::ActiveModel as CollectionEntryAM,
    issue::ActiveModel as IssueAM,
    library,
    progress_record::ActiveModel as ProgressAM,
    saved_view::ActiveModel as SavedViewAM,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, ConnectionTrait, Set};
use std::path::Path;
use uuid::Uuid;

// ───────── Library ─────────

pub struct LibrarySeed<'a> {
    pub root: &'a Path,
    pub name: Option<String>,
    pub default_reading_direction: &'static str,
    pub allow_archive_writeback: bool,
    pub metadata_writeback_enabled: bool,
}

impl<'a> LibrarySeed<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            name: None,
            default_reading_direction: "ltr",
            allow_archive_writeback: false,
            metadata_writeback_enabled: false,
        }
    }

    pub fn with_reading_direction(mut self, d: &'static str) -> Self {
        self.default_reading_direction = d;
        self
    }

    /// Flip both archive-writeback toggles on. The
    /// `metadata-sidecar-writeback-1.0` M3 apply path is only taken
    /// when both flags are true; integration tests opt in via this
    /// shortcut.
    pub fn with_sidecar_writeback(mut self) -> Self {
        self.allow_archive_writeback = true;
        self.metadata_writeback_enabled = true;
        self
    }

    pub async fn insert<C: ConnectionTrait>(self, db: &C) -> Uuid {
        let id = Uuid::now_v7();
        let now = Utc::now().fixed_offset();
        library::ActiveModel {
            id: Set(id),
            name: Set(self
                .name
                .unwrap_or_else(|| format!("Lib {}", &id.to_string()[..8]))),
            root_path: Set(self.root.to_string_lossy().into_owned()),
            default_language: Set("en".into()),
            default_reading_direction: Set(self.default_reading_direction.into()),
            dedupe_by_content: Set(true),
            slug: Set(id.to_string()),
            scan_schedule_cron: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            last_scan_at: Set(None),
            ignore_globs: Set(serde_json::json!([])),
            report_missing_comicinfo: Set(false),
            file_watch_enabled: Set(true),
            soft_delete_days: Set(30),
            thumbnails_enabled: Set(true),
            thumbnail_format: Set("webp".into()),
            thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
            thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
            generate_page_thumbs_on_scan: Set(false),
            allow_archive_writeback: Set(self.allow_archive_writeback),
            metadata_writeback_enabled: Set(self.metadata_writeback_enabled),
            archive_backup_retain_count: Set(1),
            archive_backup_retain_days: Set(30),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        }
        .insert(db)
        .await
        .unwrap();
        id
    }
}

/// Convenience: `seed_library(db, root).await -> Uuid`. Matches the
/// signature used by every OPDS-cluster test file before this module
/// existed; one-line migration.
pub async fn seed_library<C: ConnectionTrait>(db: &C, root: &Path) -> Uuid {
    LibrarySeed::new(root).insert(db).await
}

// ───────── Series ─────────

pub struct SeriesSeed<'a> {
    pub lib_id: Uuid,
    pub name: &'a str,
    pub year: Option<i32>,
    pub preserve_canonical_order: bool,
    pub reading_direction: Option<String>,
    pub publisher: Option<String>,
}

impl<'a> SeriesSeed<'a> {
    pub fn new(lib_id: Uuid, name: &'a str) -> Self {
        Self {
            lib_id,
            name,
            year: Some(2020),
            preserve_canonical_order: false,
            reading_direction: None,
            publisher: None,
        }
    }

    pub fn with_preserve_canonical_order(mut self, p: bool) -> Self {
        self.preserve_canonical_order = p;
        self
    }

    pub fn with_reading_direction(mut self, d: impl Into<String>) -> Self {
        self.reading_direction = Some(d.into());
        self
    }

    pub fn with_publisher(mut self, p: impl Into<String>) -> Self {
        self.publisher = Some(p.into());
        self
    }

    pub async fn insert<C: ConnectionTrait>(self, db: &C) -> Uuid {
        let id = Uuid::now_v7();
        let now = Utc::now().fixed_offset();
        SeriesAM {
            id: Set(id),
            library_id: Set(self.lib_id),
            name: Set(self.name.into()),
            normalized_name: Set(normalize_name(self.name)),
            year: Set(self.year),
            volume: Set(None),
            publisher: Set(self.publisher),
            imprint: Set(None),
            status: Set("continuing".into()),
            total_issues: Set(None),
            age_rating: Set(None),
            summary: Set(None),
            language_code: Set("en".into()),
            series_group: Set(None),
            slug: Set(id.to_string()),
            alternate_names: Set(serde_json::json!([])),
            // M0 metadata-providers additions; tests don't exercise
            // them today so seed with NULL/empty/false defaults.
            sort_name: Set(None),
            year_end: Set(None),
            series_type: Set(None),
            aliases: Set(serde_json::json!([])),
            deck: Set(None),
            publisher_id: Set(None),
            imprint_id: Set(None),
            last_metadata_sync_at: Set(None),
            metadata_sync_paused: Set(false),
            created_at: Set(now),
            updated_at: Set(now),
            folder_path: Set(None),
            last_scanned_at: Set(None),
            match_key: Set(None),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            status_user_set_at: Set(None),
            reading_direction: Set(self.reading_direction),
            preserve_canonical_order: Set(self.preserve_canonical_order),
        }
        .insert(db)
        .await
        .unwrap();
        id
    }
}

/// Convenience: matches `seed_series(db, lib_id, name)` shape used by
/// most OPDS tests. For `preserve_canonical_order = true`, use the
/// builder: `SeriesSeed::new(lib, name).with_preserve_canonical_order(true).insert(db).await`.
pub async fn seed_series<C: ConnectionTrait>(db: &C, lib_id: Uuid, name: &str) -> Uuid {
    SeriesSeed::new(lib_id, name).insert(db).await
}

// ───────── Issue ─────────

pub struct IssueSeed<'a> {
    pub lib_id: Uuid,
    pub series_id: Uuid,
    pub file_path: &'a Path,
    pub payload: &'a [u8],
    pub sort_number: f64,
    pub title: Option<String>,
    pub page_count: Option<i32>,
    pub state: &'static str,
}

impl<'a> IssueSeed<'a> {
    pub fn new(
        lib_id: Uuid,
        series_id: Uuid,
        file_path: &'a Path,
        payload: &'a [u8],
        sort_number: f64,
    ) -> Self {
        Self {
            lib_id,
            series_id,
            file_path,
            payload,
            sort_number,
            title: None,
            page_count: Some(20),
            state: "active",
        }
    }

    pub fn with_title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }

    pub fn with_page_count(mut self, n: i32) -> Self {
        self.page_count = Some(n);
        self
    }

    pub fn with_page_count_opt(mut self, n: Option<i32>) -> Self {
        self.page_count = n;
        self
    }

    pub fn with_state(mut self, s: &'static str) -> Self {
        self.state = s;
        self
    }

    pub async fn insert<C: ConnectionTrait>(self, db: &C) -> String {
        std::fs::write(self.file_path, self.payload).unwrap();
        let bytes = std::fs::read(self.file_path).unwrap();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let now = Utc::now().fixed_offset();
        let title = self
            .title
            .unwrap_or_else(|| format!("Issue {}", self.sort_number));
        IssueAM {
            id: Set(hash.clone()),
            library_id: Set(self.lib_id),
            series_id: Set(self.series_id),
            slug: Set(Uuid::now_v7().to_string()),
            file_path: Set(self.file_path.to_string_lossy().into_owned()),
            file_size: Set(std::fs::metadata(self.file_path).unwrap().len() as i64),
            file_mtime: Set(now),
            state: Set(self.state.into()),
            content_hash: Set(hash.clone()),
            title: Set(Some(title)),
            sort_number: Set(Some(self.sort_number)),
            number_raw: Set(Some(format!("{}", self.sort_number))),
            volume: Set(None),
            year: Set(None),
            month: Set(None),
            day: Set(None),
            summary: Set(None),
            notes: Set(None),
            language_code: Set(None),
            format: Set(None),
            black_and_white: Set(None),
            manga: Set(None),
            age_rating: Set(None),
            page_count: Set(self.page_count),
            pages: Set(serde_json::json!([])),
            comic_info_raw: Set(serde_json::json!({})),
            alternate_series: Set(None),
            story_arc: Set(None),
            story_arc_number: Set(None),
            characters: Set(None),
            teams: Set(None),
            locations: Set(None),
            tags: Set(None),
            genre: Set(None),
            writer: Set(None),
            penciller: Set(None),
            inker: Set(None),
            colorist: Set(None),
            letterer: Set(None),
            cover_artist: Set(None),
            editor: Set(None),
            translator: Set(None),
            publisher: Set(None),
            imprint: Set(None),
            scan_information: Set(None),
            community_rating: Set(None),
            review: Set(None),
            web_url: Set(None),
            // M0 metadata-providers additions; tests seed with NULL.
            deck: Set(None),
            store_date: Set(None),
            foc_date: Set(None),
            price: Set(None),
            sku: Set(None),
            staff_rating: Set(None),
            aliases: Set(serde_json::json!([])),
            last_metadata_sync_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            superseded_by: Set(None),
            special_type: Set(None),
            hash_algorithm: Set(1),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
            last_rewrite_at: Set(None),
            last_rewrite_kind: Set(None),
        cover_page_index: Set(0),
        }
        .insert(db)
        .await
        .unwrap();
        hash
    }
}

/// Convenience: matches the signature used by the OPDS test cluster
/// (`opds_default_reorder.rs`, `opds_sequential_nav.rs`,
/// `opds_personal_feeds.rs`, etc.) verbatim. One-line migration.
pub async fn seed_issue<C: ConnectionTrait>(
    db: &C,
    lib_id: Uuid,
    series_id: Uuid,
    file_path: &Path,
    payload: &[u8],
    sort_number: f64,
) -> String {
    IssueSeed::new(lib_id, series_id, file_path, payload, sort_number)
        .insert(db)
        .await
}

// ───────── Progress ─────────

/// Seed a finished progress row (`last_page = 19, percent = 1.0,
/// finished = true`). Use this when the test only cares that an issue
/// is "done" — the OPDS reorder/up-next/glyph cluster's default.
pub async fn seed_progress_finished<C: ConnectionTrait>(db: &C, user_id: Uuid, issue_id: &str) {
    seed_progress(db, user_id, issue_id, 19, 1.0, true).await
}

pub async fn seed_progress<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    issue_id: &str,
    last_page: i32,
    percent: f64,
    finished: bool,
) {
    let now = Utc::now().fixed_offset();
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(last_page),
        percent: Set(percent),
        finished: Set(finished),
        finished_at: Set(if finished { Some(now) } else { None }),
        updated_at: Set(now),
        device: Set(None),
        is_backfill: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Seed a progress row at a specific timestamp, with auto-derived
/// percent (`1.0` when finished else `0.5`). Used by feed-ordering
/// tests that pin event recency to assert sort order.
pub async fn seed_progress_at<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    issue_id: &str,
    last_page: i32,
    finished: bool,
    when: chrono::DateTime<chrono::FixedOffset>,
) {
    ProgressAM {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(last_page),
        percent: Set(if finished { 1.0 } else { 0.5 }),
        finished: Set(finished),
        finished_at: Set(if finished { Some(when) } else { None }),
        updated_at: Set(when),
        device: Set(None),
        is_backfill: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
}

// ───────── CBL ─────────

pub struct CblListSeed<'a> {
    pub owner: Uuid,
    pub name: &'a str,
    pub preserve_canonical_order: bool,
}

impl<'a> CblListSeed<'a> {
    pub fn new(owner: Uuid, name: &'a str) -> Self {
        Self {
            owner,
            name,
            preserve_canonical_order: false,
        }
    }

    pub fn with_preserve_canonical_order(mut self, p: bool) -> Self {
        self.preserve_canonical_order = p;
        self
    }

    pub async fn insert<C: ConnectionTrait>(self, db: &C) -> Uuid {
        let id = Uuid::now_v7();
        let now = Utc::now().fixed_offset();
        CblListAM {
            id: Set(id),
            owner_user_id: Set(Some(self.owner)),
            source_kind: Set("upload".into()),
            source_url: Set(None),
            catalog_source_id: Set(None),
            catalog_path: Set(None),
            github_blob_sha: Set(None),
            source_etag: Set(None),
            source_last_modified: Set(None),
            raw_sha256: Set(vec![0u8; 32]),
            raw_xml: Set("<ReadingList />".into()),
            parsed_name: Set(self.name.into()),
            parsed_matchers_present: Set(false),
            num_issues_declared: Set(None),
            description: Set(None),
            imported_at: Set(now),
            last_refreshed_at: Set(None),
            last_match_run_at: Set(None),
            refresh_schedule: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            preserve_canonical_order: Set(self.preserve_canonical_order),
        }
        .insert(db)
        .await
        .unwrap();
        id
    }
}

pub async fn seed_cbl_list<C: ConnectionTrait>(db: &C, owner: Uuid, name: &str) -> Uuid {
    CblListSeed::new(owner, name).insert(db).await
}

pub async fn seed_cbl_entry<C: ConnectionTrait>(
    db: &C,
    list_id: Uuid,
    position: i32,
    matched_issue_id: Option<&str>,
) {
    let now = Utc::now().fixed_offset();
    let status = if matched_issue_id.is_some() {
        "matched"
    } else {
        "missing"
    };
    CblEntryAM {
        id: Set(Uuid::now_v7()),
        cbl_list_id: Set(list_id),
        position: Set(position),
        series_name: Set("Seed".into()),
        issue_number: Set(position.to_string()),
        volume: Set(None),
        year: Set(None),
        cv_series_id: Set(None),
        cv_issue_id: Set(None),
        metron_series_id: Set(None),
        metron_issue_id: Set(None),
        matched_issue_id: Set(matched_issue_id.map(str::to_owned)),
        match_status: Set(status.into()),
        match_method: Set(None),
        match_confidence: Set(None),
        ambiguous_candidates: Set(None),
        user_resolved_at: Set(None),
        matched_at: Set(matched_issue_id.map(|_| now)),
    }
    .insert(db)
    .await
    .unwrap();
}

// ───────── Collections ─────────

pub struct CollectionSeed<'a> {
    pub owner: Uuid,
    pub name: &'a str,
    pub preserve_canonical_order: bool,
}

impl<'a> CollectionSeed<'a> {
    pub fn new(owner: Uuid, name: &'a str) -> Self {
        Self {
            owner,
            name,
            preserve_canonical_order: false,
        }
    }

    pub fn with_preserve_canonical_order(mut self, p: bool) -> Self {
        self.preserve_canonical_order = p;
        self
    }

    pub async fn insert<C: ConnectionTrait>(self, db: &C) -> Uuid {
        let id = Uuid::now_v7();
        let now = Utc::now().fixed_offset();
        SavedViewAM {
            id: Set(id),
            user_id: Set(Some(self.owner)),
            kind: Set("collection".into()),
            system_key: Set(None),
            name: Set(self.name.into()),
            description: Set(None),
            custom_year_start: Set(None),
            custom_year_end: Set(None),
            custom_tags: Set(Vec::new()),
            match_mode: Set(None),
            conditions: Set(None),
            sort_field: Set(None),
            sort_order: Set(None),
            result_limit: Set(None),
            cbl_list_id: Set(None),
            auto_pin: Set(false),
            preserve_canonical_order: Set(self.preserve_canonical_order),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
        id
    }
}

pub async fn seed_collection<C: ConnectionTrait>(db: &C, owner: Uuid, name: &str) -> Uuid {
    CollectionSeed::new(owner, name).insert(db).await
}

pub async fn seed_collection_entry_issue<C: ConnectionTrait>(
    db: &C,
    view_id: Uuid,
    position: i32,
    issue_id: &str,
) {
    let now = Utc::now().fixed_offset();
    CollectionEntryAM {
        id: Set(Uuid::now_v7()),
        saved_view_id: Set(view_id),
        position: Set(position),
        entry_kind: Set("issue".into()),
        series_id: Set(None),
        issue_id: Set(Some(issue_id.into())),
        added_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}
