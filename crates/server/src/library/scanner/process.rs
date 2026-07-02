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

use crate::library::event_log::{Action, Category, EventCollector, Severity};
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
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, QuerySelect, Set, Statement, sea_query::Expr,
};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;

use super::stats::ScanStats;

/// Lean per-file projection for the size+mtime fast path (audit PERF-6).
/// The full `issue::Model` carries `comic_info_raw` — often tens of KB per
/// row — and the old manifest loaded AND cloned it for every unchanged file
/// on a re-scan. The fast path only needs the fingerprint columns plus the
/// count-backfill bit, which is computed in SQL so the JSON never leaves
/// Postgres.
#[derive(FromQueryResult)]
struct RowMeta {
    file_path: String,
    file_size: i64,
    file_mtime: chrono::DateTime<chrono::FixedOffset>,
    needs_count_backfill: bool,
}

/// SQL mirror of [`row_needs_comicinfo_count_backfill`] — keep the two in
/// sync (the unit test `manifest_backfill_expr_matches_rust` pins parity).
const NEEDS_COUNT_BACKFILL_EXPR: &str = "(comicinfo_count IS NULL AND (\
     jsonb_typeof(comic_info_raw) = 'null' \
     OR (comic_info_raw ? 'count' AND comic_info_raw->'count' <> 'null'::jsonb) \
     OR (comic_info_raw ? 'Count' AND comic_info_raw->'Count' <> 'null'::jsonb)))";

pub struct IssueManifest {
    by_path: HashMap<String, RowMeta>,
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
            .select_only()
            .column(issue::Column::FilePath)
            .column(issue::Column::FileSize)
            .column(issue::Column::FileMtime)
            .column_as(
                Expr::cust(NEEDS_COUNT_BACKFILL_EXPR),
                "needs_count_backfill",
            )
            .into_model::<RowMeta>()
            .all(db)
            .await?;
        Ok(Self {
            by_path: rows
                .into_iter()
                .map(|row| (row.file_path.clone(), row))
                .collect(),
        })
    }

    /// Does a row exist for this path at all?
    pub fn contains(&self, path: &str) -> bool {
        self.by_path.contains_key(path)
    }

    /// The size+mtime fast-path check, clone-free: true when a row exists,
    /// its fingerprint matches the on-disk file, and it doesn't need the
    /// comicinfo-count backfill. Mirrors [`row_metadata_is_current`] on the
    /// projected columns.
    pub fn metadata_is_current(
        &self,
        path: &str,
        size: i64,
        mtime: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        self.by_path.get(path).is_some_and(|m| {
            m.file_size == size && m.file_mtime.to_utc() == mtime && !m.needs_count_backfill
        })
    }
}

/// Result of [`parse_archive`] — annotated so callers can route failures to
/// the right health bucket. Variants intentionally use the `Ok`/`Missing*`/etc
/// names from the original error space; clippy's "variant starts with enum
/// name" lint doesn't apply here.
#[expect(clippy::large_enum_variant)]
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

/// Side-channel data the archive crate produces alongside parse
/// outcomes. Empty in the happy path; populated when the archive
/// crate had to repair the file or drop entries during open. The
/// scanner translates these into `RecoveredArchive` /
/// `SkippedArchiveEntries` health-issues so users see them in the
/// admin Health tab.
///
/// See `~/.claude/plans/recovery-visibility-1.0.md` Tranche A.
#[derive(Debug, Default)]
struct ArchiveDiagnostics {
    /// `Some(tag)` if a recovery branch fired during open. Tag values
    /// are static strings from `archive::recovery`.
    recovery: Option<&'static str>,
    /// Entries dropped from the page index by a soft defense.
    skipped: Vec<archive::SkippedEntry>,
    /// Total entries the archive crate saw (kept + dropped). Used to
    /// render "X of Y dropped" in the health-issue payload so the
    /// operator has both the count and the ratio.
    total_entries: u32,
}

/// Per-series ingest context. Holds the immutable cluster the
/// ingest pipeline threads through; per-file inputs (`path`, the
/// optional fingerprint, the slug-set, and the mutable
/// `IngestOutputs`) stay as positional args. Introduced in
/// code-quality-cleanup M3 to close the
/// `clippy::too_many_arguments` suppression on both ingest entry
/// points.
pub struct IngestCtx<'a> {
    pub state: &'a AppState,
    pub lib: &'a library::Model,
    pub series_id: Uuid,
    /// Series folder root. Used to detect when `path` sits inside an
    /// allowlist subfolder (`Specials`/`Annuals`/…) for path-derived
    /// `special_type` classification. Pass the series folder itself
    /// even when the archive lives directly inside it.
    pub series_folder: &'a Path,
    pub manifest: Option<&'a IssueManifest>,
    /// When true, bypass the per-file size+mtime fast path so the archive
    /// is re-read and ComicInfo re-parsed even if nothing on disk changed.
    /// Used by manual "Scan issue" / "Scan series" / library force scans
    /// where the user explicitly asked for a fresh ingest (e.g. to pick up
    /// new parser fields without touching every file's mtime).
    pub force: bool,
}

/// Mutable per-batch outputs. Bundled into one struct so the call site
/// can build one `IngestOutputs` and re-pass it through the loop.
pub struct IngestOutputs<'a> {
    pub stats: &'a mut ScanStats,
    pub health: &'a mut HealthCollector,
    /// Durable per-entity manifest (observability-split M3). Issue
    /// add/update/remove/restore events are buffered here during ingest and
    /// bulk-flushed at scan finalize. Observational only — never a data write.
    pub events: &'a mut EventCollector,
}

pub async fn ingest_one<C: ConnectionTrait>(
    ctx: &IngestCtx<'_>,
    db: &C,
    path: &Path,
    // F-2: when `Some`, allocate new issue slugs against this in-memory
    // HashSet (pre-fetched once per series) instead of issuing a
    // `SELECT COUNT(*)` per candidate. The set is mutated as slugs are
    // chosen so successive archives in the same batch don't collide.
    // When `None`, fall back to the per-call DB allocator.
    slug_set: Option<&mut std::collections::HashSet<String>>,
    outputs: &mut IngestOutputs<'_>,
) -> anyhow::Result<()> {
    let (size, mtime) = file_fingerprint(path)?;
    ingest_one_with_fingerprint(ctx, db, path, slug_set, size, mtime, outputs).await
}

/// One archive's parsed state, ready for downstream column mapping.
/// The MetronInfo has already been merged into `info` (spec §4.4 +
/// §6.8 — MetronInfo wins on overlap), so callers see a single
/// canonical `ComicInfo`.
struct ParsedArchive {
    hash: String,
    info: ComicInfo,
    actual_pages: u32,
    /// `"active"` for happy-path + MissingComicInfo (no ComicInfo but
    /// the archive is intact); `"encrypted"` / `"malformed"` for the
    /// degraded paths. Stored on `issues.state` so downstream consumers
    /// (UI, page-bytes guard) can react to the partial-data state.
    parse_state: &'static str,
    /// Full MetronInfo `<ID source="...">` map when MetronInfo.xml was
    /// present. Kept separately from `info` (which only carries the
    /// legacy CV/Metron/GTIN trio) so the per-issue ingest can write
    /// every external identifier — GCD, Marvel, LoCG, ISBN, etc. —
    /// straight to `external_ids` with `SetBy::MetronInfo`.
    /// metadata-providers-1.0 M8.
    metron_ids: Option<BTreeMap<String, String>>,
    /// Whether a `MetronInfo.xml` entry was present in the archive — tracked
    /// independently of `metron_ids` (a MetronInfo with no `<ID>` block still
    /// counts as present). Persisted to `issues.metroninfo_present` for the
    /// issue Metadata tab's source-files report.
    metroninfo_present: bool,
}

/// Outcome of the by-content-hash dedupe check, when no row was found
/// via the file_path index. `Moved` and `Duplicate` both rely on the
/// fingerprint matching an existing row; the difference is whether
/// the old path is still on disk.
enum DedupeOutcome {
    NotDuplicate,
    /// Boxed because `issue::Model` is ~1KB; keeping the enum
    /// discriminant cheap to pass on the happy path.
    Moved(Box<issue::Model>),
    Duplicate,
}

/// Hash + parse the archive (under the archive-work semaphore + on a
/// blocking task), translate diagnostics into health-issues, record
/// phase timings, and unwrap the `ArchiveOutcome` into a single
/// `ParsedArchive` shape. Returns `Ok(None)` for `Unreadable` — the
/// caller has nothing to write and should return `Ok(())`.
async fn parse_archive_for_ingest(
    state: &AppState,
    lib: &library::Model,
    path: &Path,
    size: i64,
    stats: &mut ScanStats,
    health: &mut HealthCollector,
) -> anyhow::Result<Option<ParsedArchive>> {
    let path_for_blocking = path.to_path_buf();
    // F-9: tunable read buffer for BLAKE3 hashing. Larger buffers reduce
    // syscall + page-cache-readahead overhead; default 1024 KB matches the
    // historical hardcoded chunk and was the right value all along — the
    // env var existed but wasn't wired until now.
    let hash_buffer_kb = state.cfg().scan_hash_buffer_kb;
    // `ArchiveLimits` is `Copy`; capture once before spawn_blocking so a
    // `COMIC_ARCHIVE_MAX_*` env override flows into the parse path.
    let archive_limits = state.cfg().archive_limits();
    let (hash, archive_outcome, timing, diagnostics) = {
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
            let (archive_outcome, mut timing, diagnostics) =
                parse_archive_timed(&path_for_blocking, archive_limits);
            timing.hash_ms = hash_ms;
            Ok::<_, anyhow::Error>((hash, archive_outcome, timing, diagnostics))
        })
        .await
        .map_err(|e| anyhow::anyhow!("archive parse task failed: {e}"))??
    };

    // Translate archive-crate diagnostics into structured health
    // issues. Both run independently of the ComicInfo outcome — a
    // file can need recovery AND have a clean ComicInfo, or be
    // partially-skipped AND have rich metadata. See Tranche A of
    // `~/.claude/plans/recovery-visibility-1.0.md`.
    if let Some(technique) = diagnostics.recovery {
        health.emit(IssueKind::RecoveredArchive {
            path: path.to_path_buf(),
            technique: technique.to_string(),
        });
    }
    if !diagnostics.skipped.is_empty() {
        // Group by reason so a future file that trips multiple soft
        // defenses gets one row per defense. Today only the
        // compression-ratio cap can fire, so the loop typically runs
        // once.
        let mut by_reason: std::collections::HashMap<&'static str, u32> =
            std::collections::HashMap::new();
        for s in &diagnostics.skipped {
            *by_reason.entry(s.reason).or_default() += 1;
        }
        for (reason, dropped) in by_reason {
            health.emit(IssueKind::SkippedArchiveEntries {
                path: path.to_path_buf(),
                dropped,
                total: diagnostics.total_entries,
                reason: reason.to_string(),
            });
        }
    }
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
            return Ok(None);
        }
    };

    // Merge MetronInfo over ComicInfo for the overlapping fields (spec §4.4 +
    // §6.8: MetronInfo wins).
    if let Some(m) = &metron_opt {
        merge_metron_into_comicinfo(&mut info, m);
    }

    let metron_ids = metron_opt.as_ref().map(|m| m.ids.clone());
    let metroninfo_present = metron_opt.is_some();

    Ok(Some(ParsedArchive {
        hash,
        info,
        actual_pages,
        parse_state,
        metron_ids,
        metroninfo_present,
    }))
}

/// Outcome of attempting a scan-time CBR→CBZ conversion.
enum CbrIngestAction {
    /// Converted to this `.cbz`; caller should ingest it.
    Converted(std::path::PathBuf),
    /// A `.cbz` twin already existed; skip the `.cbr` without a health issue.
    SkipDuplicate,
    /// Conversion couldn't run (malformed/encrypted/IO); caller falls back
    /// to the `UnsupportedArchiveFormat` skip.
    SkipUnsupported,
}

/// Whether this library wants `.cbr` files auto-converted to `.cbz` on scan.
/// Requires the opt-in flag, the master writeback prerequisite, and a
/// writable mount (rewrites would fail mid-swap on a read-only mount).
fn cbr_conversion_eligible(lib: &library::Model) -> bool {
    lib.auto_convert_cbr_on_scan
        && lib.allow_archive_writeback
        && crate::archive_rewrite::mount_writable(Path::new(&lib.root_path))
}

/// Convert a `.cbr` to a sibling `.cbz` under the archive-work semaphore (on
/// a blocking task — RAR decompression is CPU/IO heavy). Conversion failures
/// are soft: they map to a skip rather than aborting the scan chunk. Only
/// infrastructure failures (semaphore closed, join panic) propagate.
async fn convert_cbr_for_ingest(state: &AppState, path: &Path) -> anyhow::Result<CbrIngestAction> {
    let limits = state.cfg().archive_limits();
    let src = path.to_path_buf();
    let _permit = state
        .archive_work_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| anyhow::anyhow!("archive work semaphore closed: {e}"))?;
    let result =
        tokio::task::spawn_blocking(move || super::cbr_convert::convert_cbr_to_cbz(&src, limits))
            .await
            .map_err(|e| anyhow::anyhow!("cbr convert task failed: {e}"))?;
    match result {
        Ok(dst) => {
            tracing::info!(cbr = %path.display(), cbz = %dst.display(), "scanner: converted CBR→CBZ");
            Ok(CbrIngestAction::Converted(dst))
        }
        Err(super::cbr_convert::CbrConvertError::DestinationExists(dst)) => {
            tracing::info!(
                cbr = %path.display(),
                cbz = %dst.display(),
                "scanner: .cbz twin already exists; skipping CBR conversion",
            );
            Ok(CbrIngestAction::SkipDuplicate)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "scanner: CBR→CBZ conversion failed");
            Ok(CbrIngestAction::SkipUnsupported)
        }
    }
}

/// Stamp `library.cbr_convert_confirmed_at` on first conversion so the page
/// editor stops prompting for the format change. Uses a `WHERE … IS NULL`
/// guard so it no-ops after the first conversion and is safe under the
/// parallel scan workers. Best-effort: a failure here only affects the UI
/// prompt, not the conversion itself.
async fn stamp_cbr_confirmed(state: &AppState, lib: &library::Model) {
    use sea_orm::sea_query::Expr;
    if lib.cbr_convert_confirmed_at.is_some() {
        return;
    }
    let now = Utc::now().fixed_offset();
    let res = library::Entity::update_many()
        .col_expr(library::Column::CbrConvertConfirmedAt, Expr::value(now))
        .col_expr(library::Column::UpdatedAt, Expr::value(now))
        .filter(library::Column::Id.eq(lib.id))
        .filter(library::Column::CbrConvertConfirmedAt.is_null())
        .exec(&state.db)
        .await;
    if let Err(e) = res {
        tracing::warn!(library_id = %lib.id, error = %e, "scanner: cbr_convert_confirmed_at stamp failed");
    }
}

/// Check whether the file's content fingerprint matches a row that
/// the per-path lookup didn't find. Two cases:
///
/// - The matching row's `file_path` is stale (file moved on disk):
///   stamp `issue_paths` with the old location, return `Moved` so the
///   caller updates the existing row in place.
/// - The matching row's `file_path` still exists: this is a true
///   duplicate; emit a health issue, increment the dup counter, and
///   return `Duplicate` so the caller bails before INSERT and avoids
///   a primary-key conflict that would roll back the chunk.
///
/// Filters by `content_hash` rather than `id` so retagged rows (where
/// `id` is the historical first-insert hash but diverges from the live
/// `content_hash`) are still detected as the same logical issue.
async fn dedupe_by_content_hash<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    path_str: &str,
    series_id: Uuid,
    path: &Path,
    health: &mut HealthCollector,
    stats: &mut ScanStats,
) -> anyhow::Result<DedupeOutcome> {
    let prior = IssueEntity::find()
        .filter(issue::Column::ContentHash.eq(issue_id))
        .order_by_asc(issue::Column::Id)
        .one(db)
        .await?;
    let Some(prior) = prior else {
        return Ok(DedupeOutcome::NotDuplicate);
    };
    let previous_path = std::path::Path::new(&prior.file_path);
    if prior.file_path != path_str && !previous_path.exists() {
        remember_moved_issue_path(db, &prior.id, &prior.file_path, path_str).await?;
        Ok(DedupeOutcome::Moved(Box::new(prior)))
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
        Ok(DedupeOutcome::Duplicate)
    }
}

pub async fn ingest_one_with_fingerprint<C: ConnectionTrait>(
    ctx: &IngestCtx<'_>,
    db: &C,
    path: &Path,
    slug_set: Option<&mut std::collections::HashSet<String>>,
    size: i64,
    mtime: chrono::DateTime<chrono::Utc>,
    outputs: &mut IngestOutputs<'_>,
) -> anyhow::Result<()> {
    let IngestCtx {
        state,
        lib,
        series_id,
        series_folder,
        manifest,
        force,
    } = *ctx;
    // Reborrow the mutable outputs once at the top so the function body
    // can keep referring to `stats` / `health` directly instead of
    // `outputs.stats` / `outputs.health` at every site.
    let stats: &mut ScanStats = &mut *outputs.stats;
    let health: &mut HealthCollector = &mut *outputs.health;
    let events: &mut EventCollector = &mut *outputs.events;
    let path_str = path.to_string_lossy().into_owned();

    // Existing row? If size+mtime match, skip re-hash (spec §9.1 fast path).
    //
    // PERF-6: with a manifest, the fast path runs against the projected
    // fingerprint columns — no full-`Model` load, no clone. Only files that
    // are new/changed/backfill-pending (the ones about to pay a hash +
    // archive parse anyway) fetch their full row for the update path below.
    if !force
        && let Some(manifest) = manifest
        && manifest.metadata_is_current(&path_str, size, mtime)
    {
        stats.files_unchanged += 1;
        // The archive isn't being re-opened, so any open health issue tied to
        // this file (MissingComicInfo, MalformedComicInfo, …) won't be
        // re-emitted by the rest of `process_file`. Tell the collector so the
        // auto-resolve sweep at scan end leaves it alone.
        health.touch_file(path);
        return Ok(());
    }
    let mut existing = if let Some(manifest) = manifest
        && !manifest.contains(&path_str)
    {
        // Manifest says no row exists — skip the pointless point query.
        None
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
        // No-manifest fallback fast path (single-file scans).
        stats.files_unchanged += 1;
        health.touch_file(path);
        return Ok(());
    }

    // Spec §10.1 UnsupportedArchiveFormat — a recognized extension we can't
    // ingest directly. `cb7` has no reader/writer at all. `cbr` has a
    // read-only reader: when the library opts into `auto_convert_cbr_on_scan`
    // (and writeback is enabled on a writable mount) we convert it to a
    // sibling `.cbz` in place — keeping the original as `.cbr.bak` — and
    // ingest the `.cbz` instead. Otherwise CBR is skipped with the same
    // health issue.
    let ext_lower = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    match ext_lower.as_deref() {
        Some("cbr") => {
            if cbr_conversion_eligible(lib) {
                match convert_cbr_for_ingest(state, path).await? {
                    CbrIngestAction::Converted(dst) => {
                        stats.files_converted += 1;
                        let e = events
                            .build(
                                Category::File,
                                Action::Converted,
                                Severity::Info,
                                format!(
                                    "Converted {} → {}",
                                    path.display(),
                                    dst.file_name()
                                        .map(|n| n.to_string_lossy())
                                        .unwrap_or_default()
                                ),
                            )
                            .detail(serde_json::json!({
                                "from": path.to_string_lossy(),
                                "to": dst.to_string_lossy(),
                            }));
                        events.push(e);
                        // The original `.cbr` is now `.cbr.bak` (not a
                        // recognized extension), so it won't re-enumerate and
                        // conversion never re-fires. Stamp the library's
                        // first-conversion ack so the page editor stops
                        // prompting, then ingest the fresh `.cbz` normally —
                        // it takes the usual hash/parse/insert path and
                        // creates a new issue row.
                        stamp_cbr_confirmed(state, lib).await;
                        let (size, mtime) = file_fingerprint(&dst)?;
                        return Box::pin(ingest_one_with_fingerprint(
                            ctx, db, &dst, slug_set, size, mtime, outputs,
                        ))
                        .await;
                    }
                    // A `.cbz` twin already exists — leave it to the normal
                    // path; skip the `.cbr` quietly (likely a duplicate).
                    CbrIngestAction::SkipDuplicate => {
                        stats.files_skipped += 1;
                        return Ok(());
                    }
                    // Conversion failed (malformed RAR, encrypted, IO): fall
                    // through to the same skip + health issue as when
                    // conversion is disabled.
                    CbrIngestAction::SkipUnsupported => {}
                }
            }
            stats.files_skipped += 1;
            health.emit(IssueKind::UnsupportedArchiveFormat {
                path: path.to_path_buf(),
                ext: "cbr".to_owned(),
            });
            return Ok(());
        }
        Some("cb7") => {
            stats.files_skipped += 1;
            health.emit(IssueKind::UnsupportedArchiveFormat {
                path: path.to_path_buf(),
                ext: "cb7".to_owned(),
            });
            return Ok(());
        }
        _ => {}
    }

    let Some(ParsedArchive {
        hash,
        info,
        actual_pages,
        parse_state,
        metron_ids,
        metroninfo_present,
    }) = parse_archive_for_ingest(state, lib, path, size, stats, health).await?
    else {
        return Ok(());
    };

    // Filename inference fills gaps. M7: honor per-library
    // ComicTagger-style toggles for leading-number stripping and
    // assume-issue-one.
    let leaf = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let inferred = filename::infer_with_opts(leaf, filename_opts(lib));

    let series_publisher = info.publisher.clone().or(inferred.publisher.clone());

    // Build issue. (Series identity was resolved by the caller in process_folder.)
    let number_raw = info
        .number
        .clone()
        .or(inferred.number.clone())
        .filter(|s| !s.is_empty());
    let sort_number = number_raw.as_deref().and_then(parse_sort_number);
    // Cheap clone kept for the event-log label: `number_raw` is moved into the
    // issue ActiveModel further down (both the add and update branches), so we
    // can't borrow it for the manifest summary after that point.
    let number_label = number_raw.clone();

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

    // Spec §6.5 + nested-folders M2.5: classify specials/annuals/one-shots.
    // Path-derived hint fires when the archive lives in an allowlist
    // subfolder (e.g. `Series/Specials/Artbook 1.cbz`); the immediate
    // parent's name drives the classification. Pass `None` when the
    // archive sits directly in the series folder so the existing
    // filename/format heuristics still run unchanged.
    let parent_folder_name = path
        .parent()
        .filter(|p| *p != series_folder)
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    let special_type = detect_special_type(
        info.format.as_deref(),
        leaf,
        number_raw.is_some(),
        parent_folder_name,
    )
    .map(|s| s.to_string());

    let now = Utc::now().fixed_offset();
    let issue_id = hash.clone(); // dedupe_by_content default

    // Dedupe-by-content fires only when the per-path lookup missed.
    // `order_by_asc(Id)` inside the helper keeps the choice
    // deterministic if two rows transiently share a hash.
    if existing.is_none() {
        match dedupe_by_content_hash(db, &issue_id, &path_str, series_id, path, health, stats)
            .await?
        {
            DedupeOutcome::NotDuplicate => {}
            DedupeOutcome::Moved(prior) => existing = Some(*prior),
            DedupeOutcome::Duplicate => return Ok(()),
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
        // `row.id` is the stable identifier set at first insert. Keep it
        // pinned — never `Set(...)` it — so when bytes change (retag with
        // ComicTagger, etc.) the UPDATE's WHERE clause still matches.
        // `content_hash` carries the live fingerprint instead.
        let row_id = row.id.clone();
        let mut am: IssueAM = row.into();
        am.library_id = Set(lib.id);
        am.series_id = Set(series_id);
        am.file_path = Set(path_str.clone());
        am.file_size = Set(size);
        am.file_mtime = Set(mtime.fixed_offset());
        am.state = Set(parse_state.into());
        am.content_hash = Set(hash);
        am.special_type = Set(special_type.clone());
        am.metroninfo_present = Set(Some(metroninfo_present));
        am.title = Set(info.title.clone());
        if !edited.contains("sort_number") {
            am.sort_number = Set(sort_number);
        }
        am.number_raw = Set(number_raw);
        // ComicInfo `<Volume>` and MetronInfo carry the same Mylar3
        // `V<year>` pollution as filenames. Gate every source through
        // `plausible_volume` so year-stamped values get dropped at
        // ingest — same rule already applied to `inferred.volume`.
        am.volume = Set(info
            .volume
            .filter(|&v| filename::plausible_volume(v, info.year))
            .or(inferred.volume));
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
        // M6: stamp the cover-page index from ComicInfo's
        // `<Page Type="FrontCover"/>` marker. Re-stamp on every
        // ingest so a ComicInfo edit that moves the cover triggers
        // a fresh thumbnail extraction (handled by the
        // content_changed branch below — `thumbnail_version = 0`
        // re-enqueues the post-scan thumb job which re-reads this
        // column for the page index).
        am.cover_page_index =
            Set(parsers::comicinfo::front_cover_page_index(&info.pages).unwrap_or(0));
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
        // External-ID writes (CV / Metron / GTIN from ComicInfo) move
        // to the external_ids table via writers::set_legacy_id_trio.
        // Called after the update lands (line ~669) so the issue row
        // exists. `set_by=ComicInfo` lets writers::set_external_id's
        // user-precedence rule skip rows where the user pinned a
        // different value — replacing the old `edited.contains(...)`
        // gates at this layer.
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
        // Thumbnails live under the stable `row_id` directory so they
        // survive content-hash drift (retags don't change which row the
        // thumbs belong to). The post-scan worker will re-encode pages
        // when `thumbnails_generated_at` is cleared above.
        let strip_dir =
            crate::library::thumbnails::issue_thumbs_dir(&state.cfg().data_path, &row_id);
        if content_changed && strip_dir.exists() {
            // Best-effort — a leftover stale dir isn't fatal, the worker
            // will overwrite individual files. But we want to drop pages
            // that disappeared from the new archive. `remove_dir_all` is
            // synchronous; route through `spawn_blocking` so the async
            // ingest path doesn't stall on slow / network-mounted data
            // dirs. M4 of code-quality-cleanup-1.0.
            let strip_dir_owned = strip_dir.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = std::fs::remove_dir_all(&strip_dir_owned) {
                    tracing::warn!(path = %strip_dir_owned.display(), error = %e, "failed to wipe stale strip dir");
                }
            })
            .await;
        }
        let updated = am.update(db).await?;
        remember_primary_issue_path(db, &row_id, &path_str).await?;
        // Persist CV / Metron / GTIN from ComicInfo into external_ids.
        // User-pinned values (set_by='user') are skipped automatically.
        let skips = crate::metadata::writers::set_legacy_id_trio(
            db,
            "issue",
            &updated.id,
            info.comicvine_id,
            info.metron_id,
            info.gtin.as_deref(),
            crate::metadata::writers::SetBy::ComicInfo,
        )
        .await?;
        emit_external_id_skips(health, path, skips);
        // metadata-providers-1.0 M8: MetronInfo's `<ID source="…">` list
        // carries IDs for any number of providers (GCD, Marvel, LoCG,
        // ISBN, etc.) — not just CV/Metron. Write each one so cross-
        // source matching downstream can short-circuit on a known ID
        // instead of running another search. The legacy trio above
        // already covered metron/comicvine but is restricted to
        // SetBy::ComicInfo; re-writing them under SetBy::MetronInfo
        // here is harmless (set_external_id is idempotent and the
        // user-precedence rule is symmetric across non-user set_by).
        if let Some(ids) = &metron_ids {
            let skips = write_metroninfo_external_ids(db, "issue", &updated.id, ids).await?;
            emit_external_id_skips(health, path, skips);
        }
        // F-1: pass the just-updated model directly instead of re-fetching by id.
        super::metadata_rollup::replace_issue_metadata_from_model(db, &updated).await?;
        stats.files_updated += 1;
        let label = issue_label(info.title.as_deref(), number_label.as_deref(), &updated.id);
        let e = events
            .build(
                Category::Issue,
                Action::Updated,
                Severity::Info,
                format!("Updated issue {label}"),
            )
            .entity("issue", updated.id.clone(), Some(label))
            .detail(serde_json::json!({
                "series_id": series_id,
                "content_changed": content_changed,
                "file_path": path_str,
            }));
        events.push(e);
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
            // Same plausibility gate as the update path above —
            // ComicInfo / MetronInfo `<Volume>` values in the
            // 1900–2100 year range are Mylar3 stamps, not real
            // volumes.
            volume: Set(info
                .volume
                .filter(|&v| filename::plausible_volume(v, info.year))
                .or(inferred.volume)),
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
            cover_page_index: Set(
                parsers::comicinfo::front_cover_page_index(&info.pages).unwrap_or(0)
            ),
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
            // CV / Metron / GTIN persist via writers::set_legacy_id_trio
            // after the insert lands (a few lines down) — those live
            // in the external_ids table now.
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
            special_type: Set(special_type),
            metroninfo_present: Set(Some(metroninfo_present)),
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
            // Archive rewrite bookkeeping — set by the sidecar / edit
            // workers (M3+ of metadata-sidecar-writeback-1.0 and M2+
            // of archive-rewrite-1.0). NULL for issues Folio has never
            // rewritten the bytes of.
            last_rewrite_at: Set(None),
            last_rewrite_kind: Set(None),
            // A freshly-scanned issue is never pre-accepted (B4); the operator
            // sets this later from the worklist.
            metadata_review_accepted_at: Set(None),
            metadata_review_accepted_by: Set(None),
        };
        let inserted = am.insert(db).await?;
        remember_primary_issue_path(db, &issue_id, &path_str).await?;
        let skips = crate::metadata::writers::set_legacy_id_trio(
            db,
            "issue",
            &inserted.id,
            info.comicvine_id,
            info.metron_id,
            info.gtin.as_deref(),
            crate::metadata::writers::SetBy::ComicInfo,
        )
        .await?;
        emit_external_id_skips(health, path, skips);
        // metadata-providers-1.0 M8 — see the matching block on the
        // update path above for the cross-source-IDs rationale.
        if let Some(ids) = &metron_ids {
            let skips = write_metroninfo_external_ids(db, "issue", &inserted.id, ids).await?;
            emit_external_id_skips(health, path, skips);
        }
        // F-1: pass the just-inserted model directly instead of re-fetching by id.
        super::metadata_rollup::replace_issue_metadata_from_model(db, &inserted).await?;
        stats.files_added += 1;
        let label = issue_label(info.title.as_deref(), number_label.as_deref(), &inserted.id);
        let e = events
            .build(
                Category::Issue,
                Action::Added,
                Severity::Info,
                format!("Added issue {label}"),
            )
            .entity("issue", inserted.id.clone(), Some(label))
            .detail(serde_json::json!({
                "series_id": series_id,
                "file_path": path_str,
            }));
        events.push(e);
    }

    Ok(())
}

/// Human-friendly one-line label for an issue event: the title if the
/// archive carried one, else `#<number>`, else the synthesized issue id.
fn issue_label(title: Option<&str>, number_raw: Option<&str>, issue_id: &str) -> String {
    match (title, number_raw) {
        (Some(t), _) if !t.trim().is_empty() => t.trim().to_owned(),
        (_, Some(n)) if !n.trim().is_empty() => format!("#{}", n.trim()),
        _ => issue_id.to_owned(),
    }
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

fn parse_archive_timed(
    path: &Path,
    limits: ArchiveLimits,
) -> (ArchiveOutcome, ArchiveTiming, ArchiveDiagnostics) {
    parse_archive_timed_with(path, ParseMode::FullIngest, limits)
}

fn parse_archive_timed_with(
    path: &Path,
    mode: ParseMode,
    limits: ArchiveLimits,
) -> (ArchiveOutcome, ArchiveTiming, ArchiveDiagnostics) {
    let parse_started = Instant::now();
    let mut timing = ArchiveTiming::default();
    let mut archive = match archive::open(path, limits) {
        Ok(c) => c,
        Err(ArchiveError::Encrypted) => {
            return (
                ArchiveOutcome::Encrypted,
                timing,
                ArchiveDiagnostics::default(),
            );
        }
        Err(ArchiveError::Io(s)) => {
            return (
                ArchiveOutcome::Unreadable(s),
                timing,
                ArchiveDiagnostics::default(),
            );
        }
        Err(other) => {
            return (
                ArchiveOutcome::Malformed(other.to_string()),
                timing,
                ArchiveDiagnostics::default(),
            );
        }
    };

    // Capture archive-crate diagnostics now while the box is in
    // scope — these signals describe what `open()` had to do to make
    // the file readable, independent of any later ComicInfo parsing
    // outcome. Cloned to detach from the archive's borrow.
    let diagnostics = ArchiveDiagnostics {
        recovery: archive.recovery_used(),
        skipped: archive.entries_skipped().to_vec(),
        total_entries: (archive.entries().len() + archive.entries_skipped().len()) as u32,
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
    (outcome, timing, diagnostics)
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
    // Gate MetronInfo's `<volume>` through the same plausibility
    // filter — newer schema, same potential for tooling pollution.
    if let Some(v) = m.volume
        && filename::plausible_volume(v, m.year)
    {
        info.volume = Some(v);
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

    // MetronInfo carries structured list fields (`<Team>`, `<Character>`,
    // …); ComicInfo collapses them to a single `<Teams>…</Teams>` CSV. A
    // name like "Capes, Inc." is ambiguous under any `,`-split. When
    // MetronInfo is present we trust its boundaries and feed them
    // through the same smart joiner the sidecar composer uses — `; `
    // when any name contains a comma, else `, ` — so the rollup's
    // `split_csv` (which splits on both) recovers the correct list.
    if !m.teams.is_empty() {
        info.teams = Some(join_csv_unambiguous(&m.teams));
    }
    if !m.characters.is_empty() {
        info.characters = Some(join_csv_unambiguous(&m.characters));
    }
    if !m.locations.is_empty() {
        info.locations = Some(join_csv_unambiguous(&m.locations));
    }
    if !m.tags.is_empty() {
        info.tags = Some(join_csv_unambiguous(&m.tags));
    }
    if !m.genres.is_empty() {
        info.genre = Some(join_csv_unambiguous(&m.genres));
    }
    if !m.story_arcs.is_empty() {
        info.story_arc = Some(join_csv_unambiguous(&m.story_arcs));
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

/// Mirror of `sidecar_compose::join_unambiguous_csv`. Joins with `; `
/// when any name contains a comma so the rollup's `split_csv` recovers
/// the structured boundaries instead of fragmenting `"Capes, Inc."`.
fn join_csv_unambiguous(names: &[String]) -> String {
    let sep = if names.iter().any(|n| n.contains(',')) {
        "; "
    } else {
        ", "
    };
    names.join(sep)
}

/// Spec §6.5 special_type detection. Heuristics applied in order:
///   1. `ComicInfo.Format` field — explicit author signal wins
///   2. Parent folder name (if set) matches the series-subfolder
///      allowlist (`Specials`/`Annuals`/`Oneshots`/...) — see
///      [`enumerate::is_series_subfolder_name`]. Closes the
///      `Series/Specials/Artbook 1.cbz` ↔ `Series - v01.cbz`
///      numbering collision documented in
///      `~/.claude/plans/scanner-nested-folders-1.0.md` M2.5.
///   3. Filename token: `Annual`, `_Annual_` → Annual
///   4. Filename token: `_SP_`, `Special` → Special
///   5. No recognizable issue number → OneShot
///   6. Otherwise → None
///
/// `parent_folder_name` should be the *immediate* parent folder's
/// name (`Path::file_name()`) when the archive lives in a subfolder of
/// the series folder, and `None` when it lives at the series folder
/// itself. Callers in the scanner derive this by comparing
/// `path.parent()` to the series folder.
pub fn detect_special_type(
    format: Option<&str>,
    filename: &str,
    has_number: bool,
    parent_folder_name: Option<&str>,
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
    if let Some(name) = parent_folder_name
        && let Some(tag) = special_type_from_subfolder(name)
    {
        return Some(tag);
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

/// Map a series-subfolder name (case-insensitive) to the
/// `special_type` it implies. Returns `None` for names outside the
/// allowlist, including the canonical "main run lives here" case
/// where the archive sits directly in the series folder.
fn special_type_from_subfolder(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "specials" | "extras" | "bonus" | "tie-ins" => Some("Special"),
        "annuals" | "annual" => Some("Annual"),
        "oneshots" | "one-shots" => Some("OneShot"),
        _ => None,
    }
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
/// Build a [`filename::InferOpts`] from the library row. Matching-
/// accuracy-1.0 M7. Both flags default OFF; operators flip them
/// per-library via the settings card.
pub(crate) fn filename_opts(lib: &library::Model) -> filename::InferOpts {
    filename::InferOpts {
        ignore_leading_numbers: lib.filename_ignore_leading_numbers,
        assume_issue_one: lib.filename_assume_issue_one,
    }
}

pub fn peek_identity_hint(
    path: &Path,
    limits: ArchiveLimits,
    infer_opts: filename::InferOpts,
) -> SeriesIdentityHint {
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
    let inferred = filename::infer_with_opts(leaf, infer_opts);

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
    let resolved_year = info.year.or(inferred.year);
    SeriesIdentityHint {
        series_name,
        year: resolved_year,
        // Identity-time volume signal: ComicInfo gated through
        // `plausible_volume`, then filename inference (already gated
        // at parser level), then the folder-leaf V-token fallback
        // applied in `process_planned_folder`.
        volume: info
            .volume
            .filter(|&v| filename::plausible_volume(v, resolved_year))
            .or(inferred.volume),
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

/// Detect identifier tags in a series folder name —
/// `\[([a-z]{2,6})-(\d+)\]` resolved against the [`Source`] prefix
/// registry. Pre-tagged libraries (metron-tagger output, ComicTagger,
/// hand-tagged folders) emit one or more `[cv-12345]`, `[metron-67890]`,
/// `[gcd-…]` etc. tokens in the folder name. Each detected token
/// becomes an `external_ids` row on the series, letting the metadata-
/// providers matcher short-circuit on a known ID instead of running a
/// full search.
///
/// Source-agnostic by design: adding a new prefix needs only an entry
/// in `Source::from_str` (`crates/server/src/metadata/identifier.rs`).
/// Unknown prefixes are dropped silently. Mixed-case (`[CV-...]`) is
/// normalized by `Source::from_str`. Embedded tags (`Saga [cv-12345] (2012)`)
/// + trailing tags both work since the regex doesn't anchor.
///
/// metadata-providers-1.0 M8.
pub fn parse_series_folder_tags(folder_name: &str) -> Vec<crate::metadata::identifier::Identifier> {
    use crate::metadata::identifier::{Identifier, Source};
    // Hand-rolled to avoid adding a regex dep for one scanner pattern.
    // Walks the string once; on `[` looks for prefix-then-hyphen-then-
    // digits-then-`]`. Anything off-pattern is skipped, never panicked.
    let bytes = folder_name.as_bytes();
    let mut out: Vec<Identifier> = Vec::new();
    let mut seen: std::collections::HashSet<Source> = std::collections::HashSet::new();
    let mut i = 0;
    // 2..=12 covers every alias in `Source::from_str` (longest is
    // `mangaupdates` at 12). The plan's spec text wrote {2,6} but
    // immediately listed `comicvine` (9 chars) and `metron` (6) as
    // canonical — widening to 12 honors the alias list, which is the
    // real contract operators write to.
    const PREFIX_MIN: usize = 2;
    const PREFIX_MAX: usize = 12;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut p = start;
        while p < bytes.len() && p - start < PREFIX_MAX && bytes[p].is_ascii_alphabetic() {
            p += 1;
        }
        let prefix_len = p - start;
        if !(PREFIX_MIN..=PREFIX_MAX).contains(&prefix_len) || p >= bytes.len() || bytes[p] != b'-'
        {
            i = start;
            continue;
        }
        let id_start = p + 1;
        let mut q = id_start;
        while q < bytes.len() && bytes[q].is_ascii_digit() {
            q += 1;
        }
        if q == id_start || q >= bytes.len() || bytes[q] != b']' {
            i = start;
            continue;
        }
        // Safe: prefix + id are ASCII slices we just walked byte-wise.
        let prefix = std::str::from_utf8(&bytes[start..p]).unwrap_or_default();
        let id = std::str::from_utf8(&bytes[id_start..q]).unwrap_or_default();
        match prefix.parse::<Source>() {
            Ok(source) => {
                // Dedupe by source — a folder with multiple tags for
                // the same provider keeps the first occurrence.
                // Subsequent rows would collide on the
                // (entity, source) unique key anyway; explicit skip
                // keeps the write path quiet.
                if seen.insert(source) {
                    out.push(Identifier::new(source, id));
                }
            }
            Err(_) => {
                // Unknown prefix — accept silently per the plan
                // ("unknown prefix ignored"). Operator can grep the
                // trace log if they suspect a typo.
                tracing::debug!(prefix, "scanner: ignoring unknown folder-tag prefix");
            }
        }
        i = q + 1;
    }
    out
}

/// Persist the [`parse_series_folder_tags`] output onto the series
/// row. Called once per `process_folder`, after series identity is
/// resolved. User-pinned values (`set_by='user'`) are protected by
/// `set_external_id`'s precedence rule.
///
/// metadata-providers-1.0 M8.
pub async fn write_series_folder_tags<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    folder_name: &str,
) -> Result<usize, sea_orm::DbErr> {
    let identifiers = parse_series_folder_tags(folder_name);
    let count = identifiers.len();
    let series_id_str = series_id.to_string();
    for identifier in identifiers {
        crate::metadata::writers::set_external_id(
            db,
            "series",
            &series_id_str,
            &identifier,
            crate::metadata::writers::SetBy::ScannerFolderTag,
        )
        .await?;
    }
    Ok(count)
}

/// Persist every MetronInfo `<ID source="...">` row as an
/// `external_ids` entry. Unknown sources (anything `Source::FromStr`
/// doesn't recognize) are skipped silently — a future MetronInfo
/// extension that adds a new source key shouldn't blow up the
/// scanner.
///
/// User-pinned values (`set_by='user'`) are protected by
/// `set_external_id`'s precedence rule, so this re-write is safe to
/// run on every rescan.
///
/// metadata-providers-1.0 M8.
/// Raise a `DuplicateExternalId` health finding for each provider ID a
/// write skipped because a live issue already owns it (the duplicate /
/// variant-file case). The scan still completes; the operator sees the
/// dups in `/admin/findings` instead of a wedged, retrying scan.
fn emit_external_id_skips(
    health: &mut HealthCollector,
    path: &Path,
    skips: Vec<crate::metadata::writers::SkippedExternalId>,
) {
    for s in skips {
        health.emit(IssueKind::DuplicateExternalId {
            path: path.to_path_buf(),
            source: s.source.as_str().to_owned(),
            external_id: s.external_id,
            owner_entity_id: s.owner,
        });
    }
}

async fn write_metroninfo_external_ids<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    ids: &BTreeMap<String, String>,
) -> Result<Vec<crate::metadata::writers::SkippedExternalId>, sea_orm::DbErr> {
    let mut skipped = Vec::new();
    for (raw_source, raw_id) in ids {
        let trimmed = raw_id.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(source) = raw_source.parse::<crate::metadata::identifier::Source>() else {
            tracing::debug!(
                entity_id,
                source = raw_source,
                "scanner: ignoring MetronInfo ID with unknown source"
            );
            continue;
        };
        let identifier = crate::metadata::identifier::Identifier::new(source, trimmed);
        if let crate::metadata::writers::SetExternalIdOutcome::SkippedConflict { owner } =
            crate::metadata::writers::set_external_id(
                db,
                entity_type,
                entity_id,
                &identifier,
                crate::metadata::writers::SetBy::MetronInfo,
            )
            .await?
        {
            skipped.push(crate::metadata::writers::SkippedExternalId {
                source,
                external_id: trimmed.to_owned(),
                owner,
            });
        }
    }
    Ok(skipped)
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

    #[test]
    fn detect_special_type_uses_subfolder_when_filename_silent() {
        // Filename has a number, no recognizable token, but the
        // parent folder is `Specials` → Special wins.
        assert_eq!(
            detect_special_type(None, "Artbook 1.cbz", true, Some("Specials")),
            Some("Special"),
        );
        assert_eq!(
            detect_special_type(None, "Vol 2024.cbz", true, Some("Annuals")),
            Some("Annual"),
        );
        assert_eq!(
            detect_special_type(None, "Ashcan 1.cbz", true, Some("Oneshots")),
            Some("OneShot"),
        );
        // Allowlist is case-insensitive.
        assert_eq!(
            detect_special_type(None, "Artbook 1.cbz", true, Some("SPECIALS")),
            Some("Special"),
        );
        // Non-allowlist subfolder name doesn't trigger the path rule.
        assert_eq!(
            detect_special_type(None, "Foo 001.cbz", true, Some("Volume 2")),
            None,
        );
    }

    #[test]
    fn detect_special_type_format_beats_subfolder() {
        // ComicInfo Format wins over path hint.
        assert_eq!(
            detect_special_type(Some("Annual"), "Artbook 1.cbz", true, Some("Specials")),
            Some("Annual"),
        );
    }

    #[test]
    fn detect_special_type_subfolder_beats_filename_token() {
        // The `_sp_` filename token would normally classify as Special.
        // The Annuals parent must win because path-derived runs before
        // filename heuristics.
        assert_eq!(
            detect_special_type(None, "x_sp_y 001.cbz", true, Some("Annuals")),
            Some("Annual"),
        );
    }

    #[test]
    fn detect_special_type_no_parent_falls_back_to_filename() {
        // No parent means the archive sits directly in the series
        // folder. Filename heuristics still run.
        assert_eq!(
            detect_special_type(None, "Series Annual 2024.cbz", true, None),
            Some("Annual"),
        );
        assert_eq!(
            detect_special_type(None, "Series Origin.cbz", false, None),
            Some("OneShot"),
        );
        assert_eq!(
            detect_special_type(None, "Series 001.cbz", true, None),
            None
        );
    }

    // ─────────────────────────────────────────────────────────────
    // metadata-providers-1.0 M8 — folder-name identifier tags.
    // ─────────────────────────────────────────────────────────────

    fn tag_pair(folder: &str) -> Vec<(String, String)> {
        parse_series_folder_tags(folder)
            .into_iter()
            .map(|i| (i.source.as_str().to_owned(), i.id))
            .collect()
    }

    #[test]
    fn folder_tags_recognize_canonical_prefixes() {
        // Every prefix the plan calls out (`cv`, `comicvine`, `metron`,
        // `gcd`, `marvel`, `locg`) resolves through the same
        // `Source::from_str` registry the rest of the codebase uses.
        assert_eq!(
            tag_pair("Saga (2012) [cv-12345]"),
            vec![("comicvine".into(), "12345".into())]
        );
        assert_eq!(
            tag_pair("Saga (2012) [comicvine-12345]"),
            vec![("comicvine".into(), "12345".into())]
        );
        assert_eq!(
            tag_pair("Saga (2012) [metron-67890]"),
            vec![("metron".into(), "67890".into())]
        );
        assert_eq!(
            tag_pair("Saga (2012) [gcd-111]"),
            vec![("gcd".into(), "111".into())]
        );
        assert_eq!(
            tag_pair("Saga (2012) [marvel-222]"),
            vec![("marvel".into(), "222".into())]
        );
        assert_eq!(
            tag_pair("Saga (2012) [locg-333]"),
            vec![("locg".into(), "333".into())]
        );
    }

    #[test]
    fn folder_tags_mixed_case_normalizes() {
        // Source::from_str lowercases internally, so authors writing
        // `[CV-...]`, `[Comicvine-...]`, `[METRON-...]` all land on
        // the same Source — operators copy-paste from a variety of
        // tools and we shouldn't punish stylistic drift.
        assert_eq!(
            tag_pair("Saga [CV-12345]"),
            vec![("comicvine".into(), "12345".into())]
        );
        assert_eq!(
            tag_pair("Saga [Comicvine-12345]"),
            vec![("comicvine".into(), "12345".into())]
        );
        assert_eq!(
            tag_pair("Saga [METRON-67890]"),
            vec![("metron".into(), "67890".into())]
        );
    }

    #[test]
    fn folder_tags_multiple_sources_in_one_name() {
        let pairs = tag_pair("Saga (2012) [cv-12345] [metron-67890] [gcd-111]");
        assert_eq!(pairs.len(), 3);
        assert!(pairs.contains(&("comicvine".into(), "12345".into())));
        assert!(pairs.contains(&("metron".into(), "67890".into())));
        assert!(pairs.contains(&("gcd".into(), "111".into())));
    }

    #[test]
    fn folder_tags_embedded_vs_trailing() {
        // Tags anywhere in the string are accepted — embedded between
        // words (`Saga [cv-12345] (2012)`) and trailing
        // (`Saga (2012) [cv-12345]`) are both common authoring styles.
        assert_eq!(
            tag_pair("Saga [cv-12345] (2012)"),
            vec![("comicvine".into(), "12345".into())]
        );
        assert_eq!(
            tag_pair("[cv-12345] Saga (2012)"),
            vec![("comicvine".into(), "12345".into())]
        );
    }

    #[test]
    fn folder_tags_unknown_prefix_silently_ignored() {
        // Per the plan: "unknown prefix ignored". A typo or a
        // not-yet-supported source shouldn't break ingest.
        assert_eq!(tag_pair("Saga (2012) [foo-12345]"), Vec::new());
        assert_eq!(
            tag_pair("Saga (2012) [xy-12345] [cv-1]"),
            vec![("comicvine".into(), "1".into())]
        );
    }

    #[test]
    fn folder_tags_malformed_tokens_skipped() {
        // Off-pattern tokens — empty id, alpha id, missing hyphen,
        // unclosed bracket — must not panic or yield bogus rows.
        assert_eq!(tag_pair("Saga [cv-]"), Vec::new());
        assert_eq!(tag_pair("Saga [cv-abc]"), Vec::new());
        assert_eq!(tag_pair("Saga [cv12345]"), Vec::new());
        assert_eq!(tag_pair("Saga [cv-12345"), Vec::new());
        assert_eq!(tag_pair("Saga cv-12345]"), Vec::new());
        assert_eq!(tag_pair("Saga (2012)"), Vec::new());
        assert_eq!(tag_pair(""), Vec::new());
    }

    #[test]
    fn folder_tags_dedupe_same_source() {
        // Two tags for the same provider in one folder name keep the
        // first occurrence — the (entity, source) unique key on
        // external_ids would reject the second anyway; explicit dedupe
        // keeps the write loop quiet.
        let pairs = tag_pair("Saga [cv-1] [cv-2]");
        assert_eq!(pairs, vec![("comicvine".into(), "1".into())]);
    }
}
