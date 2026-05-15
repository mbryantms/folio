//! Per-file processing pipeline (spec §6).
//!
//! Hash → existence check → parse → series identity → upsert → cover thumb.
//! Lifted verbatim from the pre-Milestone-3 scanner.rs; behavior parity is
//! enforced by [`scan_dispatch`](../../../tests/scan_dispatch.rs) plus the
//! existing thumbnails / page_bytes integration tests, which still drive
//! the public `scan_library` API.
//!
//! Milestone 8 expands this with series.json / MetronInfo / volume
//! disambiguation / specials detection / hash-mismatch supersession.

use crate::library::health::{HealthCollector, IssueKind};
use crate::library::identity::SeriesIdentityHint;
use crate::state::AppState;
use archive::{ArchiveError, ArchiveLimits};
use chrono::Utc;
use entity::{
    issue::{self, ActiveModel as IssueAM, Entity as IssueEntity},
    library,
};
use parsers::{
    comicinfo::{ComicInfo, PageInfo},
    filename,
    metroninfo::MetronInfo,
    series_json::SeriesMetadata,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, Statement,
};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;

use super::stats::ScanStats;

pub struct IssueManifest {
    by_path: HashMap<String, issue::Model>,
}

impl IssueManifest {
    pub async fn for_paths<C: ConnectionTrait>(
        db: &C,
        paths: &[std::path::PathBuf],
    ) -> anyhow::Result<Self> {
        let path_strings = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        if path_strings.is_empty() {
            return Ok(Self {
                by_path: HashMap::new(),
            });
        }
        let rows = IssueEntity::find()
            .filter(issue::Column::FilePath.is_in(path_strings))
            .all(db)
            .await?;
        Ok(Self {
            by_path: rows
                .into_iter()
                .map(|row| (row.file_path.clone(), row))
                .collect(),
        })
    }

    pub fn by_path(&self, path: &str) -> Option<issue::Model> {
        self.by_path.get(path).cloned()
    }
}

/// Result of [`parse_archive`] — annotated so callers can route failures to
/// the right health bucket. Variants intentionally use the `Ok`/`Missing*`/etc
/// names from the original error space; clippy's "variant starts with enum
/// name" lint doesn't apply here.
#[allow(clippy::large_enum_variant)]
enum ArchiveOutcome {
    /// ComicInfo and (optionally) MetronInfo extracted. MetronInfo wins on
    /// overlapping fields when merged downstream (spec §4.4). `actual_pages`
    /// is the count of image entries in the archive — used to synthesize
    /// missing page metadata without trusting ComicInfo PageCount.
    Ok {
        info: ComicInfo,
        metron: Option<MetronInfo>,
        actual_pages: u32,
    },
    /// No ComicInfo.xml in the archive. `info.pages` is synthesized from the
    /// dimension probe so the reader still gets per-page metadata (image
    /// width/height + inferred `double_page`) — without it the reader would
    /// have no way to know which pages are spreads.
    MissingComicInfo {
        info: ComicInfo,
        actual_pages: u32,
    },
    Malformed(String),
    Encrypted,
    Unreadable(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseMode {
    FullIngest,
    IdentityOnly,
}

impl ParseMode {
    fn probe_dimensions(self) -> bool {
        matches!(self, Self::FullIngest)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct ArchiveTiming {
    hash_ms: u64,
    archive_parse_ms: u64,
    page_probe_ms: u64,
}

#[allow(clippy::too_many_arguments)]
pub async fn ingest_one<C: ConnectionTrait>(
    state: &AppState,
    db: &C,
    lib: &library::Model,
    path: &Path,
    series_id: Uuid,
    manifest: Option<&IssueManifest>,
    // F-2: when `Some`, allocate new issue slugs against this in-memory
    // HashSet (pre-fetched once per series) instead of issuing a
    // `SELECT COUNT(*)` per candidate. The set is mutated as slugs are
    // chosen so successive archives in the same batch don't collide.
    // When `None`, fall back to the per-call DB allocator.
    slug_set: Option<&mut std::collections::HashSet<String>>,
    stats: &mut ScanStats,
    health: &mut HealthCollector,
    force: bool,
) -> anyhow::Result<()> {
    let (size, mtime) = file_fingerprint(path)?;
    ingest_one_with_fingerprint(
        state, db, lib, path, series_id, manifest, slug_set, size, mtime, stats, health, force,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn ingest_one_with_fingerprint<C: ConnectionTrait>(
    state: &AppState,
    db: &C,
    lib: &library::Model,
    path: &Path,
    series_id: Uuid,
    manifest: Option<&IssueManifest>,
    // See `ingest_one` for `slug_set` semantics.
    slug_set: Option<&mut std::collections::HashSet<String>>,
    size: i64,
    mtime: chrono::DateTime<chrono::Utc>,
    stats: &mut ScanStats,
    health: &mut HealthCollector,
    // When true, bypass the per-file size+mtime fast path so the archive
    // is re-read and ComicInfo re-parsed even if nothing on disk changed.
    // Used by manual "Scan issue" / "Scan series" / library force scans
    // where the user explicitly asked for a fresh ingest (e.g. to pick up
    // new parser fields without touching every file's mtime).
    force: bool,
) -> anyhow::Result<()> {
    // Postgres `timestamptz` truncates writes to microsecond precision, so a
    // fresh nanosecond-precision fs mtime never round-trips byte-equal. Force
    // both sides into the same precision up front so the §9.1 fast path
    // actually fires on subsequent scans of unchanged files. (Without this,
    // every per-series rescan re-hashes, re-parses, and re-thumbs every file
    // even when nothing on disk changed.)
    let path_str = path.to_string_lossy().into_owned();

    // Existing row? If size+mtime match, skip re-hash (spec §9.1 fast path).
    let mut existing = if let Some(manifest) = manifest {
        manifest.by_path(&path_str)
    } else {
        IssueEntity::find()
            .filter(issue::Column::FilePath.eq(path_str.clone()))
            .one(db)
            .await?
    };
    if !force
        && let Some(row) = &existing
        && row_metadata_is_current(row, size, mtime)
    {
        stats.files_unchanged += 1;
        // The archive isn't being re-opened, so any open health issue tied to
        // this file (MissingComicInfo, MalformedComicInfo, …) won't be
        // re-emitted by the rest of `process_file`. Tell the collector so the
        // auto-resolve sweep at scan end leaves it alone.
        health.touch_file(path);
        return Ok(());
    }

    // Spec §10.1 UnsupportedArchiveFormat — recognized extension but no
    // reader yet (currently CBR + CB7 — see crates/archive/src/cbr.rs and
    // cb7.rs). Emit a health issue and skip without trying to hash a file
    // we can't read.
    let ext_lower = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(ext_lower.as_deref(), Some("cbr" | "cb7")) {
        stats.files_skipped += 1;
        health.emit(IssueKind::UnsupportedArchiveFormat {
            path: path.to_path_buf(),
            ext: ext_lower.unwrap_or_default(),
        });
        return Ok(());
    }

    // Hash + parse. Both operations are blocking filesystem/archive work, so keep
    // them off the async scheduler used by HTTP and websocket handlers.
    let path_for_blocking = path.to_path_buf();
    // F-9: tunable read buffer for BLAKE3 hashing. Larger buffers reduce
    // syscall + page-cache-readahead overhead; default 1024 KB matches the
    // historical hardcoded chunk and was the right value all along — the
    // env var existed but wasn't wired until now.
    let hash_buffer_kb = state.cfg().scan_hash_buffer_kb;
    // `ArchiveLimits` is `Copy`; capture once before spawn_blocking so
    // a `COMIC_ARCHIVE_MAX_*` env override flows into the parse path.
    let archive_limits = state.cfg().archive_limits();
    let (hash, archive_outcome, timing) = {
        let _archive_permit = state
            .archive_work_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("archive work semaphore closed: {e}"))?;
        tokio::task::spawn_blocking(move || {
            let hash_started = Instant::now();
            let hash =
                crate::library::hash::blake3_file_with_buffer(&path_for_blocking, hash_buffer_kb)?;
            let hash_ms = hash_started.elapsed().as_millis() as u64;
            let (archive_outcome, mut timing) =
                parse_archive_timed(&path_for_blocking, archive_limits);
            timing.hash_ms = hash_ms;
            Ok::<_, anyhow::Error>((hash, archive_outcome, timing))
        })
        .await
        .map_err(|e| anyhow::anyhow!("archive parse task failed: {e}"))??
    };
    stats.record_phase_parallel("hash", std::time::Duration::from_millis(timing.hash_ms));
    stats.record_phase_parallel(
        "archive_parse",
        std::time::Duration::from_millis(timing.archive_parse_ms),
    );
    stats.record_phase_parallel(
        "page_probe",
        std::time::Duration::from_millis(timing.page_probe_ms),
    );
    stats.record_bytes_hashed(size.max(0) as u64);
    let (mut info, metron_opt, actual_pages, parse_state) = match archive_outcome {
        ArchiveOutcome::Ok {
            info,
            metron,
            actual_pages,
        } => (info, metron, actual_pages, "active"),
        ArchiveOutcome::MissingComicInfo { info, actual_pages } => {
            // §11: only emit a health issue when explicitly requested. Mylar/CBL
            // libraries usually have ComicInfo everywhere; loose libraries
            // don't, and we don't want to spam them.
            if lib.report_missing_comicinfo {
                health.emit(IssueKind::MissingComicInfo {
                    path: path.to_path_buf(),
                });
            }
            (info, None, actual_pages, "active")
        }
        ArchiveOutcome::Encrypted => {
            stats.files_encrypted += 1;
            (ComicInfo::default(), None, 0, "encrypted")
        }
        ArchiveOutcome::Malformed(e) => {
            tracing::warn!(path = %path.display(), error = %e, "malformed archive");
            stats.files_malformed += 1;
            health.emit(IssueKind::MalformedComicInfo {
                path: path.to_path_buf(),
                error: e.clone(),
            });
            (ComicInfo::default(), None, 0, "malformed")
        }
        ArchiveOutcome::Unreadable(e) => {
            tracing::warn!(path = %path.display(), error = %e, "io error reading archive");
            stats.files_skipped += 1;
            health.emit(IssueKind::UnreadableArchive {
                path: path.to_path_buf(),
                error: e,
            });
            return Ok(());
        }
    };

    // Merge MetronInfo over ComicInfo for the overlapping fields (spec §4.4 +
    // §6.8: MetronInfo wins).
    if let Some(m) = &metron_opt {
        merge_metron_into_comicinfo(&mut info, m);
    }

    // Filename inference fills gaps.
    let leaf = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let inferred = filename::infer(leaf);

    let series_publisher = info.publisher.clone().or(inferred.publisher.clone());

    // Build issue. (Series identity was resolved by the caller in process_folder.)
    let number_raw = info
        .number
        .clone()
        .or(inferred.number.clone())
        .filter(|s| !s.is_empty());
    let sort_number = number_raw.as_deref().and_then(parse_sort_number);

    let comic_info_raw = serde_json::to_value(&info)?;
    let pages_json = serde_json::to_value(&info.pages)?;

    // Authoritative page count: the actual number of image entries we found
    // in the archive, not ComicInfo `<PageCount>`. Publisher metadata is
    // sometimes off by one (extra cover slot, unused trailing entry, etc.),
    // and the strip-thumbnail worker can only encode pages that exist on
    // disk — so trusting `<PageCount>` makes the readiness denominator
    // chase a number that can never be reached. The original ComicInfo
    // value is still preserved verbatim in `comic_info_raw` for reference.
    // For unreadable / encrypted / malformed archives `actual_pages == 0`,
    // and we fall back to whatever ComicInfo claimed (likely also missing).
    let resolved_page_count = if actual_pages > 0 {
        Some(actual_pages as i32)
    } else {
        info.page_count
    };

    // Spec §6.5: classify specials/annuals/one-shots.
    let special_type = detect_special_type(info.format.as_deref(), leaf, number_raw.is_some())
        .map(|s| s.to_string());

    let now = Utc::now().fixed_offset();
    let issue_id = hash.clone(); // dedupe_by_content default

    // Dedupe-by-content: when the file_path lookup misses but a row already
    // exists with this hash as its id, treat it as a move if the old path is
    // gone; otherwise surface a duplicate warning instead of rolling back the
    // chunk on a primary-key conflict.
    if existing.is_none() {
        let prior = IssueEntity::find_by_id(issue_id.clone()).one(db).await?;
        if let Some(prior) = prior {
            let previous_path = std::path::Path::new(&prior.file_path);
            if prior.file_path != path_str && !previous_path.exists() {
                remember_moved_issue_path(db, &issue_id, &prior.file_path, &path_str).await?;
                existing = Some(prior);
            } else {
                health.emit(IssueKind::DuplicateContent {
                    path_a: std::path::PathBuf::from(&prior.file_path),
                    path_b: path.to_path_buf(),
                });
                stats.files_duplicate += 1;
                tracing::info!(
                    series_id = %series_id,
                    kept = %prior.file_path,
                    duplicate = %path.display(),
                    "scanner: skipping duplicate (same content hash as existing issue)",
                );
                return Ok(());
            }
        }
    }

    if let Some(row) = existing {
        // Fields the user has overridden via `PATCH /issues/{id}` are sticky:
        // the scanner refreshes everything else from ComicInfo but leaves
        // these alone. Cheap O(n) check on a tiny array.
        let edited = user_edited_set(&row.user_edited);
        // Track whether the actual file contents changed. Force scans go
        // through this branch even when size+mtime match — in that case
        // there's no point invalidating thumbnails, since the rendered
        // pages haven't changed. Decided here so the conditional below
        // doesn't have to inspect the active model.
        let content_changed = !row_matches_file(&row, size, mtime);
        let mut am: IssueAM = row.into();
        am.id = Set(issue_id.clone());
        am.library_id = Set(lib.id);
        am.series_id = Set(series_id);
        am.file_path = Set(path_str.clone());
        am.file_size = Set(size);
        am.file_mtime = Set(mtime.fixed_offset());
        am.state = Set(parse_state.into());
        am.content_hash = Set(hash);
        am.special_type = Set(special_type.clone());
        am.title = Set(info.title.clone());
        if !edited.contains("sort_number") {
            am.sort_number = Set(sort_number);
        }
        am.number_raw = Set(number_raw);
        am.volume = Set(info.volume.or(inferred.volume));
        am.year = Set(info.year);
        am.month = Set(info.month);
        am.day = Set(info.day);
        am.summary = Set(info.summary.clone());
        am.notes = Set(info.notes.clone());
        if !edited.contains("language_code") {
            am.language_code = Set(info.language_iso.clone());
        }
        am.format = Set(info.format.clone());
        am.black_and_white = Set(info.black_and_white);
        am.manga = Set(info.manga.clone());
        if !edited.contains("age_rating") {
            am.age_rating = Set(info.age_rating.clone());
        }
        am.page_count = Set(resolved_page_count);
        am.pages = Set(pages_json);
        am.comic_info_raw = Set(comic_info_raw);
        am.alternate_series = Set(info.alternate_series.clone());
        am.story_arc = Set(info.story_arc.clone());
        am.story_arc_number = Set(info.story_arc_number.clone());
        am.characters = Set(info.characters.clone());
        am.teams = Set(info.teams.clone());
        am.locations = Set(info.locations.clone());
        if !edited.contains("tags") {
            am.tags = Set(info.tags.clone());
        }
        if !edited.contains("genre") {
            am.genre = Set(info.genre.clone());
        }
        am.writer = Set(info.writer.clone());
        am.penciller = Set(info.penciller.clone());
        am.inker = Set(info.inker.clone());
        am.colorist = Set(info.colorist.clone());
        am.letterer = Set(info.letterer.clone());
        am.cover_artist = Set(info.cover_artist.clone());
        am.editor = Set(info.editor.clone());
        am.translator = Set(info.translator.clone());
        am.publisher = Set(info.publisher.clone().or(series_publisher.clone()));
        am.imprint = Set(info.imprint.clone());
        am.scan_information = Set(info.scan_information.clone());
        am.community_rating = Set(info.community_rating);
        am.review = Set(info.review.clone());
        am.web_url = Set(info.web.clone());
        am.gtin = Set(info.gtin.clone());
        if !edited.contains("comicvine_id") {
            am.comicvine_id = Set(info.comicvine_id);
        }
        if !edited.contains("metron_id") {
            am.metron_id = Set(info.metron_id);
        }
        // ComicInfo `<Count>` per-issue. The reconcile step computes
        // a per-series MAX over this column to refresh
        // `series.total_issues` and derive `status`.
        am.comicinfo_count = Set(info.count);
        am.updated_at = Set(now);
        // Only invalidate thumbnails when the file's bytes actually changed.
        // Force scans hit this branch on size+mtime-equal files too; those
        // have identical pages, so re-thumbing would be pure waste.
        if content_changed {
            am.thumbnails_generated_at = Set(None);
            am.thumbnail_version = Set(0);
            am.thumbnails_error = Set(None);
        }
        let strip_dir =
            crate::library::thumbnails::issue_thumbs_dir(&state.cfg().data_path, &issue_id);
        if content_changed && strip_dir.exists() {
            // Best-effort — a leftover stale dir isn't fatal, the worker
            // will overwrite individual files. But we want to drop pages
            // that disappeared from the new archive.
            if let Err(e) = std::fs::remove_dir_all(&strip_dir) {
                tracing::warn!(path = %strip_dir.display(), error = %e, "failed to wipe stale strip dir");
            }
        }
        let updated = am.update(db).await?;
        remember_primary_issue_path(db, &issue_id, &path_str).await?;
        // F-1: pass the just-updated model directly instead of re-fetching by id.
        super::metadata_rollup::replace_issue_metadata_from_model(db, &updated).await?;
        stats.files_updated += 1;
    } else {
        let issue_slug = if let Some(set) = slug_set {
            crate::slug::allocate_issue_slug_in_set(
                set,
                number_raw.as_deref(),
                info.title.as_deref(),
                &issue_id,
            )
        } else {
            crate::slug::allocate_issue_slug(
                db,
                series_id,
                number_raw.as_deref(),
                info.title.as_deref(),
                &issue_id,
            )
            .await?
        };
        let am = IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib.id),
            series_id: Set(series_id),
            slug: Set(issue_slug),
            file_path: Set(path_str.clone()),
            file_size: Set(size),
            file_mtime: Set(mtime.fixed_offset()),
            state: Set(parse_state.into()),
            content_hash: Set(hash),
            title: Set(info.title.clone()),
            sort_number: Set(sort_number),
            number_raw: Set(number_raw),
            volume: Set(info.volume.or(inferred.volume)),
            year: Set(info.year),
            month: Set(info.month),
            day: Set(info.day),
            summary: Set(info.summary.clone()),
            notes: Set(info.notes.clone()),
            language_code: Set(info.language_iso.clone()),
            format: Set(info.format.clone()),
            black_and_white: Set(info.black_and_white),
            manga: Set(info.manga.clone()),
            age_rating: Set(info.age_rating.clone()),
            page_count: Set(resolved_page_count),
            pages: Set(pages_json),
            comic_info_raw: Set(comic_info_raw),
            alternate_series: Set(info.alternate_series.clone()),
            story_arc: Set(info.story_arc.clone()),
            story_arc_number: Set(info.story_arc_number.clone()),
            characters: Set(info.characters.clone()),
            teams: Set(info.teams.clone()),
            locations: Set(info.locations.clone()),
            tags: Set(info.tags.clone()),
            genre: Set(info.genre.clone()),
            writer: Set(info.writer.clone()),
            penciller: Set(info.penciller.clone()),
            inker: Set(info.inker.clone()),
            colorist: Set(info.colorist.clone()),
            letterer: Set(info.letterer.clone()),
            cover_artist: Set(info.cover_artist.clone()),
            editor: Set(info.editor.clone()),
            translator: Set(info.translator.clone()),
            publisher: Set(info.publisher.clone().or(series_publisher)),
            imprint: Set(info.imprint.clone()),
            scan_information: Set(info.scan_information.clone()),
            community_rating: Set(info.community_rating),
            review: Set(info.review.clone()),
            web_url: Set(info.web.clone()),
            comicvine_id: Set(info.comicvine_id),
            metron_id: Set(info.metron_id),
            gtin: Set(info.gtin.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            superseded_by: Set(None),
            special_type: Set(special_type),
            hash_algorithm: Set(1),
            // M1: post-scan thumbs worker stamps these on success.
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            // ComicInfo `<Count>` — see the update path above for why
            // we capture this per-issue.
            comicinfo_count: Set(info.count),
        };
        let inserted = am.insert(db).await?;
        remember_primary_issue_path(db, &issue_id, &path_str).await?;
        // F-1: pass the just-inserted model directly instead of re-fetching by id.
        super::metadata_rollup::replace_issue_metadata_from_model(db, &inserted).await?;
        stats.files_added += 1;
    }

    Ok(())
}

async fn remember_moved_issue_path<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    old_path: &str,
    new_path: &str,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"INSERT INTO issue_paths (issue_id, file_path, is_primary, missing_at)
            VALUES ($1, $2, false, NOW())
            ON CONFLICT (file_path) DO UPDATE
            SET issue_id = EXCLUDED.issue_id,
                is_primary = false,
                missing_at = COALESCE(issue_paths.missing_at, NOW())",
        [issue_id.into(), old_path.into()],
    ))
    .await?;
    remember_primary_issue_path(db, issue_id, new_path).await
}

async fn remember_primary_issue_path<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    file_path: &str,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"UPDATE issue_paths
            SET is_primary = false
            WHERE issue_id = $1 AND file_path <> $2",
        [issue_id.into(), file_path.into()],
    ))
    .await?;
    db.execute(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"INSERT INTO issue_paths (issue_id, file_path, is_primary, missing_at)
            VALUES ($1, $2, true, NULL)
            ON CONFLICT (file_path) DO UPDATE
            SET issue_id = EXCLUDED.issue_id,
                is_primary = true,
                missing_at = NULL",
        [issue_id.into(), file_path.into()],
    ))
    .await?;
    Ok(())
}

pub fn file_fingerprint(path: &Path) -> anyhow::Result<(i64, chrono::DateTime<chrono::Utc>)> {
    let meta = std::fs::metadata(path)?;
    let mtime: chrono::DateTime<chrono::Utc> = truncate_to_micros(meta.modified()?.into());
    Ok((meta.len() as i64, mtime))
}

pub fn row_matches_file(
    row: &issue::Model,
    size: i64,
    mtime: chrono::DateTime<chrono::Utc>,
) -> bool {
    row.file_size == size && row.file_mtime.to_utc() == mtime
}

pub fn row_metadata_is_current(
    row: &issue::Model,
    size: i64,
    mtime: chrono::DateTime<chrono::Utc>,
) -> bool {
    row_matches_file(row, size, mtime) && !row_needs_comicinfo_count_backfill(row)
}

fn row_needs_comicinfo_count_backfill(row: &issue::Model) -> bool {
    if row.comicinfo_count.is_some() {
        return false;
    }
    if row.comic_info_raw.is_null() {
        // Missing raw XML means the row predates the richer parser state; do
        // one metadata refresh so future fast paths can trust the row.
        return true;
    }
    row.comic_info_raw
        .get("count")
        .is_some_and(|v| !v.is_null())
        || row
            .comic_info_raw
            .get("Count")
            .is_some_and(|v| !v.is_null())
}

fn parse_archive_with(path: &Path, mode: ParseMode, limits: ArchiveLimits) -> ArchiveOutcome {
    parse_archive_timed_with(path, mode, limits).0
}

fn parse_archive_timed(path: &Path, limits: ArchiveLimits) -> (ArchiveOutcome, ArchiveTiming) {
    parse_archive_timed_with(path, ParseMode::FullIngest, limits)
}

fn parse_archive_timed_with(
    path: &Path,
    mode: ParseMode,
    limits: ArchiveLimits,
) -> (ArchiveOutcome, ArchiveTiming) {
    let parse_started = Instant::now();
    let mut timing = ArchiveTiming::default();
    let mut archive = match archive::open(path, limits) {
        Ok(c) => c,
        Err(ArchiveError::Encrypted) => return (ArchiveOutcome::Encrypted, timing),
        Err(ArchiveError::Io(s)) => return (ArchiveOutcome::Unreadable(s), timing),
        Err(other) => return (ArchiveOutcome::Malformed(other.to_string()), timing),
    };

    // Count image entries in the archive so missing/partial ComicInfo page
    // metadata can be synthesized without trusting ComicInfo PageCount.
    let actual_pages = archive.pages().len() as u32;

    // ComicInfo (primary) — its absence isn't fatal; we'll fall back to
    // filename inference and MetronInfo if present.
    let info_result = if archive.find("ComicInfo.xml").is_some() {
        match archive.read_entry_bytes("ComicInfo.xml") {
            Ok(bytes) => match parsers::comicinfo::parse(&bytes) {
                Ok(info) => Some(Ok(info)),
                Err(e) => Some(Err(format!("ComicInfo.xml parse: {e}"))),
            },
            Err(e) => Some(Err(format!("ComicInfo.xml read: {e}"))),
        }
    } else {
        None
    };

    // MetronInfo (sidecar) — best-effort.
    let metron = if archive.find("MetronInfo.xml").is_some() {
        archive
            .read_entry_bytes("MetronInfo.xml")
            .ok()
            .and_then(|bytes| parsers::metroninfo::parse(&bytes).ok())
    } else {
        None
    };

    let should_probe = mode.probe_dimensions()
        && match &info_result {
            Some(Ok(info)) => comicinfo_needs_dimension_probe(info, actual_pages),
            Some(Err(_)) => false,
            None => actual_pages > 0,
        };
    // Full ingestion probes page dimensions only when metadata lacks enough
    // per-page width/height/spread data. Many well-tagged ComicInfo files
    // already carry this, and re-reading every page prefix dominates warm
    // forced scans for no user-visible gain.
    let probed_dims = if should_probe {
        let probe_started = Instant::now();
        let dims = probe_page_dimensions(&mut *archive);
        timing.page_probe_ms = probe_started.elapsed().as_millis() as u64;
        dims
    } else {
        Vec::new()
    };
    timing.archive_parse_ms = parse_started
        .elapsed()
        .as_millis()
        .saturating_sub(timing.page_probe_ms as u128) as u64;

    let outcome = match info_result {
        Some(Ok(mut info)) => {
            if should_probe {
                apply_dimension_probe(&mut info, &probed_dims, actual_pages);
            }
            ArchiveOutcome::Ok {
                info,
                metron,
                actual_pages,
            }
        }
        Some(Err(e)) => ArchiveOutcome::Malformed(e),
        None if metron.is_some() => {
            let mut info = ComicInfo::default();
            if should_probe {
                apply_dimension_probe(&mut info, &probed_dims, actual_pages);
            }
            ArchiveOutcome::Ok {
                info,
                metron,
                actual_pages,
            }
        }
        None => {
            let mut info = ComicInfo::default();
            if should_probe {
                apply_dimension_probe(&mut info, &probed_dims, actual_pages);
            }
            ArchiveOutcome::MissingComicInfo { info, actual_pages }
        }
    };
    (outcome, timing)
}

fn comicinfo_needs_dimension_probe(info: &ComicInfo, actual_pages: u32) -> bool {
    if actual_pages == 0 {
        return false;
    }
    if info.pages.is_empty() {
        return true;
    }
    if (info.pages.len() as u32) < actual_pages {
        return true;
    }
    info.pages.iter().any(|page| {
        page.image_width.is_none() || page.image_height.is_none() || page.double_page.is_none()
    })
}

/// Aspect-ratio threshold above which a page is treated as a double-page
/// spread when the publisher didn't declare it. Validated against the
/// Geiger 004 fixture (singles ≈ 0.65, spreads ≈ 1.30); 1.2 catches every
/// real spread we've measured (US-comic, manga, A-series European) while
/// excluding occasional landscape singles.
const SPREAD_INFER_ASPECT: f32 = 1.2;
const DIMENSION_PROBE_BYTES: usize = 256 * 1024;

/// Read each archive page just far enough to get its pixel dimensions.
/// Returns one entry per page in the archive's natural sort order; `None`
/// for pages we couldn't read or couldn't decode.
fn probe_page_dimensions(archive: &mut dyn archive::ComicArchive) -> Vec<Option<(u32, u32)>> {
    // Collect names first so we don't hold an immutable borrow on the
    // archive (`pages()` returns &ArchiveEntry refs) while also calling
    // the &mut method `read_entry_prefix`.
    let names: Vec<String> = archive.pages().iter().map(|e| e.name.clone()).collect();
    names
        .iter()
        .map(|name| {
            let bytes = archive
                .read_entry_prefix(name, DIMENSION_PROBE_BYTES)
                .ok()?;
            image_dimensions_from_prefix(&bytes)
        })
        .collect()
}

fn image_dimensions_from_prefix(bytes: &[u8]) -> Option<(u32, u32)> {
    png_dimensions(bytes)
        .or_else(|| gif_dimensions(bytes))
        .or_else(|| webp_dimensions(bytes))
        .or_else(|| jpeg_dimensions(bytes))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 || !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return None;
    }
    let w = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
    let h = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
    Some((w, h))
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let fourcc = &bytes[12..16];
    match fourcc {
        b"VP8X" if bytes.len() >= 30 => {
            let w = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
            let h = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
            Some((w, h))
        }
        b"VP8L" if bytes.len() >= 25 && bytes[20] == 0x2f => {
            let b0 = bytes[21] as u32;
            let b1 = bytes[22] as u32;
            let b2 = bytes[23] as u32;
            let b3 = bytes[24] as u32;
            let w = 1 + (((b1 & 0x3f) << 8) | b0);
            let h = 1 + ((b3 << 6) | (b2 << 2) | ((b1 & 0xc0) >> 6));
            Some((w, h))
        }
        b"VP8 " if bytes.len() >= 30 => {
            let start = 20;
            if bytes[start + 3..start + 6] != [0x9d, 0x01, 0x2a] {
                return None;
            }
            let w = u16::from_le_bytes(bytes[start + 6..start + 8].try_into().ok()?) as u32;
            let h = u16::from_le_bytes(bytes[start + 8..start + 10].try_into().ok()?) as u32;
            Some((w & 0x3fff, h & 0x3fff))
        }
        _ => None,
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        while i < bytes.len() && bytes[i] == 0xff {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let marker = bytes[i];
        i += 1;
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        if i + 2 > bytes.len() {
            return None;
        }
        let len = u16::from_be_bytes(bytes[i..i + 2].try_into().ok()?) as usize;
        if len < 2 || i + len > bytes.len() {
            return None;
        }
        let is_sof = matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        );
        if is_sof {
            if len < 7 {
                return None;
            }
            let h = u16::from_be_bytes(bytes[i + 3..i + 5].try_into().ok()?) as u32;
            let w = u16::from_be_bytes(bytes[i + 5..i + 7].try_into().ok()?) as u32;
            return Some((w, h));
        }
        i += len;
    }
    None
}

/// Merge probed dimensions into `info.pages`. When `info.pages` already has
/// entries (from ComicInfo), backfill missing width/height and infer
/// `double_page` for entries where the publisher left it null. When pages
/// are empty (no ComicInfo, no MetronInfo), synthesize one entry per
/// archive page so the reader gets per-page metadata to iterate.
fn apply_dimension_probe(info: &mut ComicInfo, probed: &[Option<(u32, u32)>], actual_pages: u32) {
    if info.pages.is_empty() {
        // Synthesize. Grow to the archive's actual page count even when
        // some probes failed — a missing dim is a soft error (decoder
        // didn't recognize the format), not a missing page.
        let n = actual_pages as usize;
        info.pages = (0..n)
            .map(|i| {
                let dim = probed.get(i).copied().flatten();
                page_from_probe(i, dim)
            })
            .collect();
        return;
    }
    // Augment existing ComicInfo pages by `image` index (NOT positional).
    for page in info.pages.iter_mut() {
        let idx = page.image;
        if idx < 0 {
            continue;
        }
        let Some(dim) = probed.get(idx as usize).copied().flatten() else {
            continue;
        };
        if page.image_width.is_none() {
            page.image_width = Some(dim.0 as i32);
        }
        if page.image_height.is_none() {
            page.image_height = Some(dim.1 as i32);
        }
        if page.double_page.is_none()
            && let Some(infer) = infer_double_page(dim)
        {
            page.double_page = Some(infer);
            // Only flag as inferred when we POSITIVELY identified a spread.
            // Inferred-`false` is the boring no-news case and matches the
            // synthesize-from-probe path (`page_from_probe`) — keeps the
            // flag's meaning unambiguous: "we made up a yes".
            if infer {
                page.double_page_inferred = Some(true);
            }
        }
    }
}

fn page_from_probe(image: usize, dim: Option<(u32, u32)>) -> PageInfo {
    let (w, h) = match dim {
        Some(d) => (Some(d.0 as i32), Some(d.1 as i32)),
        None => (None, None),
    };
    let inferred_double = dim.and_then(infer_double_page);
    PageInfo {
        image: image as i32,
        kind: None,
        double_page: inferred_double,
        image_size: None,
        key: None,
        bookmark: None,
        image_width: w,
        image_height: h,
        // Only mark as inferred when we actually got a yes — null and
        // false both leave this None.
        double_page_inferred: if inferred_double == Some(true) {
            Some(true)
        } else {
            None
        },
    }
}

fn infer_double_page(dim: (u32, u32)) -> Option<bool> {
    let (w, h) = dim;
    if h == 0 {
        return None;
    }
    let ratio = w as f32 / h as f32;
    Some(ratio >= SPREAD_INFER_ASPECT)
}

/// Apply MetronInfo's stronger metadata over the ComicInfo defaults
/// (spec §4.4 + §6.8 — MetronInfo wins where both populate the same field).
fn merge_metron_into_comicinfo(info: &mut ComicInfo, m: &MetronInfo) {
    if m.title.is_some() {
        info.title = m.title.clone();
    }
    if m.series.is_some() {
        info.series = m.series.clone();
    }
    if m.publisher.is_some() {
        info.publisher = m.publisher.clone();
    }
    if m.imprint.is_some() {
        info.imprint = m.imprint.clone();
    }
    if m.number.is_some() {
        info.number = m.number.clone();
    }
    if m.volume.is_some() {
        info.volume = m.volume;
    }
    if m.year.is_some() {
        info.year = m.year;
    }
    if m.month.is_some() {
        info.month = m.month;
    }
    if m.day.is_some() {
        info.day = m.day;
    }
    if m.summary.is_some() {
        info.summary = m.summary.clone();
    }
    if m.notes.is_some() {
        info.notes = m.notes.clone();
    }
    if m.age_rating.is_some() {
        info.age_rating = m.age_rating.clone();
    }
    if m.language.is_some() {
        info.language_iso = m.language.clone();
    }
    if m.manga.is_some() {
        info.manga = m.manga.clone();
    }
    if m.gtin.is_some() {
        info.gtin = m.gtin.clone();
    }

    // Role-tagged credits collapse into ComicInfo's flat strings.
    if let Some(w) = m.writer() {
        info.writer = Some(w);
    }
    if let Some(w) = m.penciller() {
        info.penciller = Some(w);
    }
    if let Some(w) = m.inker() {
        info.inker = Some(w);
    }
    if let Some(w) = m.colorist() {
        info.colorist = Some(w);
    }
    if let Some(w) = m.letterer() {
        info.letterer = Some(w);
    }
    if let Some(w) = m.cover_artist() {
        info.cover_artist = Some(w);
    }
    if let Some(w) = m.editor() {
        info.editor = Some(w);
    }
    if let Some(w) = m.translator() {
        info.translator = Some(w);
    }
}

/// Spec §6.5 special_type detection. Heuristics applied in order:
///   1. `ComicInfo.Format` field — explicit signal wins
///   2. Filename token: `_SP_`, `Special` → Special
///   3. Filename token: `Annual`, `_Annual_` → Annual
///   4. No recognizable issue number → OneShot
///   5. Otherwise → None
pub fn detect_special_type(
    format: Option<&str>,
    filename: &str,
    has_number: bool,
) -> Option<&'static str> {
    if let Some(fmt) = format {
        let lc = fmt.to_ascii_lowercase();
        match lc.as_str() {
            "special" => return Some("Special"),
            "one-shot" | "oneshot" | "one_shot" => return Some("OneShot"),
            "annual" => return Some("Annual"),
            "tpb" | "trade paperback" => return Some("TPB"),
            "graphic novel" | "gn" => return Some("TPB"),
            _ => {}
        }
    }
    let lower = filename.to_ascii_lowercase();
    if lower.contains("annual") {
        return Some("Annual");
    }
    if lower.contains("_sp_") || lower.contains(" sp ") || lower.contains("special") {
        return Some("Special");
    }
    if !has_number {
        return Some("OneShot");
    }
    None
}

/// Read `series.json` from the folder root, if present. Returns `None`
/// silently when the file is missing or malformed (a malformed series.json
/// shouldn't block scanning — the spec calls for it as a hint).
pub fn read_series_json(folder: &Path) -> Option<SeriesMetadata> {
    let p = folder.join("series.json");
    let bytes = std::fs::read(&p).ok()?;
    match parsers::series_json::parse(&bytes) {
        Ok(s) => Some(s.metadata),
        Err(e) => {
            tracing::warn!(path = %p.display(), error = %e, "series.json parse failed; ignoring");
            None
        }
    }
}

/// Sample one archive in a folder for identity-resolution input. This is the
/// "first file's ComicInfo + filename inference" hint the spec §7.1 resolution
/// pipeline runs against; if the file can't be opened or has no ComicInfo,
/// filename inference still gives us a usable Series name (or the literal
/// "Unknown Series" fallback).
pub fn peek_identity_hint(path: &Path, limits: ArchiveLimits) -> SeriesIdentityHint {
    let info = match parse_archive_with(path, ParseMode::IdentityOnly, limits) {
        ArchiveOutcome::Ok { info, metron, .. } => {
            // Apply MetronInfo precedence so the identity hint reflects the
            // strongest available metadata.
            let mut info = info;
            if let Some(m) = metron.as_ref() {
                merge_metron_into_comicinfo(&mut info, m);
            }
            info
        }
        _ => ComicInfo::default(),
    };
    let leaf = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let inferred = filename::infer(leaf);

    let series_name = info
        .series
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if !inferred.series.is_empty() {
                inferred.series.clone()
            } else {
                "Unknown Series".to_string()
            }
        });
    SeriesIdentityHint {
        series_name,
        year: info.year.or(inferred.year),
        volume: info.volume.or(inferred.volume),
        publisher: info.publisher.clone().or(inferred.publisher.clone()),
        imprint: info.imprint.clone(),
        language: info.language_iso.clone(),
        age_rating: info.age_rating.clone(),
        series_group: info.series_group.clone(),
        total_issues: info.count,
        comicvine_id: info.comicvine_series_id,
        metron_id: info.metron_series_id,
        explicit_match_key: None,
    }
}

/// Parse a ComicInfo `Number` value into a sortable f64.
/// Examples: "1" → 1.0, "1.5" → 1.5, "Annual 1" → 1.0 (best-effort).
pub fn parse_sort_number(s: &str) -> Option<f64> {
    let trimmed: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse().ok()
    }
}

/// Round a `DateTime<Utc>` down to microsecond precision so values written
/// to and read back from Postgres `timestamptz` compare byte-equal.
///
/// Filesystems on Linux (ext4, btrfs, xfs) and macOS (APFS) report mtime
/// with nanosecond precision; Postgres only stores microseconds. Without
/// this normalization, `row.file_mtime == fresh_fs_mtime` is always false
/// for files whose mtime has any sub-microsecond bits — which on a modern
/// FS is essentially every file — and the scanner's per-file fast path
/// silently never fires.
fn truncate_to_micros(t: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    let micros = t.timestamp_micros();
    chrono::DateTime::<chrono::Utc>::from_timestamp_micros(micros)
        .expect("timestamp_micros round-trip in the supported chrono range")
}

/// Decode the `issue.user_edited` JSON column into a hash set of column
/// names. Tolerant of empty / malformed JSON (returns an empty set), since a
/// missing flag just means "scanner is allowed to refresh this field".
fn user_edited_set(value: &serde_json::Value) -> std::collections::HashSet<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_number_parses_numerics() {
        assert_eq!(parse_sort_number("1"), Some(1.0));
        assert_eq!(parse_sort_number("1.5"), Some(1.5));
        assert_eq!(parse_sort_number("100"), Some(100.0));
        assert_eq!(parse_sort_number("Annual 1"), Some(1.0));
        assert_eq!(parse_sort_number("not-a-number"), None);
    }

    #[test]
    fn truncate_to_micros_drops_nanos() {
        // 2026-01-15T08:30:45.123456789 → ...123456 after truncation.
        let with_nanos =
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_768_530_645, 123_456_789)
                .expect("valid timestamp");
        let truncated = truncate_to_micros(with_nanos);
        assert_eq!(truncated.timestamp_micros(), 1_768_530_645_123_456);
        // The discarded nanoseconds register as zero in the result's ns field.
        assert_eq!(
            truncated.timestamp_subsec_nanos(),
            123_456_000,
            "sub-microsecond bits must be cleared",
        );
    }

    #[test]
    fn truncate_to_micros_is_idempotent() {
        // Already at microsecond precision — must round-trip byte-equal.
        let micros_only =
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_768_530_645, 123_456_000)
                .expect("valid timestamp");
        assert_eq!(truncate_to_micros(micros_only), micros_only);
    }

    #[test]
    fn user_edited_set_handles_shapes() {
        assert!(user_edited_set(&serde_json::json!([])).is_empty());
        assert!(user_edited_set(&serde_json::json!(null)).is_empty());
        assert!(user_edited_set(&serde_json::json!({"genre": true})).is_empty());
        let s = user_edited_set(&serde_json::json!(["genre", "tags", 7]));
        assert!(s.contains("genre"));
        assert!(s.contains("tags"));
        assert_eq!(s.len(), 2);
    }
}
