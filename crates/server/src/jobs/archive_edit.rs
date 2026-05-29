//! `ArchiveEditJob` — operator-initiated page-byte edits
//! (`archive-rewrite-1.0` M2).
//!
//! Mirrors [`crate::jobs::rewrite_sidecars`]: claim the per-issue rewrite
//! mutex, rebuild the archive atomically, invalidate the zip-LRU, clear
//! the thumbnail stamps, stamp `last_rewrite_kind='edit'`, audit, and
//! enqueue a scoped rescan so the scanner re-ingests the new bytes (the
//! content-hash dedupe keeps `issue.id` stable).
//!
//! ## Page-op model
//!
//! Ops are applied **sequentially** to a working page list, each
//! addressing the *current* positions (0-based) at the time it runs — so
//! a `Reorder` permutes whatever survives previous `Remove`s, exactly
//! like a sequence of array operations. The final list is emitted under
//! contiguous names (`p0001.…`) by [`archive::cbz_write::rebuild_pages`],
//! so the reader's natural sort reproduces the requested order.
//!
//! Kept pages stream-copy their compressed bytes verbatim (no
//! recompress). Rotated / replaced pages are decoded, transformed, and
//! re-encoded — JPEG at the per-library `archive_writeback_jpeg_quality`,
//! everything else losslessly as PNG (the `image` crate can't encode
//! WebP). Existing `ComicInfo.xml` / `MetronInfo.xml` sidecars are
//! preserved verbatim; other non-page trash is dropped on rewrite, the
//! same as the sidecar path.

use crate::archive_rewrite::{self, RewriteError, mutex};
use crate::audit::{self, AuditEntry};
use crate::jobs::archive_transforms::{TransformStep, apply_chain};
use crate::state::AppState;
use apalis::prelude::*;
use archive::cbr::Cbr;
use archive::cbt::Cbt;
use archive::cbz::Cbz;
use archive::cbz_write::{OutputPage, PageBytes};
use archive::comic_archive::ComicArchive;
use archive::{ArchiveError, cbt_write, cbz_write};
use chrono::Utc;
use entity::issue;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Editable archive container formats. CB7 has no writer and isn't
/// editable; CBR is editable only via conversion to CBZ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditFormat {
    Cbz,
    Cbt,
    Cbr,
}

impl EditFormat {
    fn from_path(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("cbz") => Some(Self::Cbz),
            Some("cbt") => Some(Self::Cbt),
            Some("cbr") => Some(Self::Cbr),
            _ => None,
        }
    }
}

/// 90-degree rotation steps. Clockwise.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rot {
    R90,
    R180,
    R270,
}

impl Rot {
    fn degrees(self) -> u16 {
        match self {
            Rot::R90 => 90,
            Rot::R180 => 180,
            Rot::R270 => 270,
        }
    }
}

/// One page-level operation. Ordinals are 0-based positions in the page
/// list *as it stands when the op is applied* (ops are sequential).
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageOp {
    /// Drop the page at `ordinal`.
    Remove { ordinal: u32 },
    /// Replace the whole page list order with `new_order` — a permutation
    /// of `0..current_len`.
    Reorder { new_order: Vec<u32> },
    /// Rotate the page at `ordinal` by `degrees` (clockwise).
    Rotate { ordinal: u32, degrees: Rot },
    /// Replace the page at `ordinal` with a previously-staged upload.
    Replace { ordinal: u32, image_id: Uuid },
    /// Apply an image-adjustment chain (brightness/contrast, levels,
    /// sharpen, despeckle, crop) to the page at `ordinal`.
    Transform {
        ordinal: u32,
        chain: Vec<TransformStep>,
    },
}

/// Validation failure for a `Vec<PageOp>` against a page count. Surfaced
/// to the API as a 422 (the endpoint validates before enqueueing; the
/// worker re-validates as defense in depth).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpError {
    #[error("op {op_index}: ordinal {ordinal} out of range (page count {len})")]
    OrdinalOutOfRange {
        op_index: usize,
        ordinal: u32,
        len: usize,
    },
    #[error("op {op_index}: reorder is not a permutation of 0..{len}")]
    BadPermutation { op_index: usize, len: usize },
    #[error("no pages would remain after the requested edits")]
    EmptyResult,
}

/// Pure simulation of a `Vec<PageOp>` over `page_count` pages. Validates
/// bounds + permutations and returns the surviving page count. Shared by
/// the dry-run/validation path in the API handler and the worker. Does
/// not touch bytes.
pub fn simulate_ops(page_count: usize, ops: &[PageOp]) -> Result<usize, OpError> {
    // Track only the count + identity of surviving positions; the byte
    // resolution happens later. We model the working list as a Vec of
    // opaque slots.
    let mut slots: Vec<usize> = (0..page_count).collect();
    for (op_index, op) in ops.iter().enumerate() {
        match op {
            PageOp::Remove { ordinal } => {
                let o = *ordinal as usize;
                if o >= slots.len() {
                    return Err(OpError::OrdinalOutOfRange {
                        op_index,
                        ordinal: *ordinal,
                        len: slots.len(),
                    });
                }
                slots.remove(o);
            }
            PageOp::Reorder { new_order } => {
                let len = slots.len();
                if !is_permutation(new_order, len) {
                    return Err(OpError::BadPermutation { op_index, len });
                }
                slots = new_order.iter().map(|&i| slots[i as usize]).collect();
            }
            PageOp::Rotate { ordinal, .. }
            | PageOp::Replace { ordinal, .. }
            | PageOp::Transform { ordinal, .. } => {
                let o = *ordinal as usize;
                if o >= slots.len() {
                    return Err(OpError::OrdinalOutOfRange {
                        op_index,
                        ordinal: *ordinal,
                        len: slots.len(),
                    });
                }
            }
        }
    }
    if slots.is_empty() {
        return Err(OpError::EmptyResult);
    }
    Ok(slots.len())
}

fn is_permutation(order: &[u32], len: usize) -> bool {
    if order.len() != len {
        return false;
    }
    let mut seen = vec![false; len];
    for &i in order {
        let i = i as usize;
        if i >= len || seen[i] {
            return false;
        }
        seen[i] = true;
    }
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchiveEditJob {
    pub issue_id: String,
    pub ops: Vec<PageOp>,
    pub actor_id: Option<Uuid>,
    pub actor_ip: Option<String>,
    pub actor_ua: Option<String>,
}

pub async fn handle(job: ArchiveEditJob, state: Data<AppState>) -> Result<(), Error> {
    let state: AppState = (*state).clone();

    let mut redis = state.jobs.redis.clone();
    let claimed = match mutex::try_claim(&mut redis, &job.issue_id, mutex::EDIT_TTL_SECS).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(issue_id = %job.issue_id, error = %e, "archive edit: mutex claim failed");
            return Ok(()); // soft-fail; operator can retry
        }
    };
    if !claimed {
        tracing::info!(issue_id = %job.issue_id, "archive edit: mutex busy; skipping");
        return Ok(());
    }

    let outcome = edit_one_issue(&state, &job).await;
    let mut redis = state.jobs.redis.clone();
    mutex::release(&mut redis, &job.issue_id).await;

    audit_edit(&state, &job, &outcome).await;

    if let Ok(ref r) = outcome
        && let Err(e) =
            enqueue_scoped_rescan(&state, &r.library_id, &r.series_id, &job.issue_id).await
    {
        tracing::error!(issue_id = %job.issue_id, error = %e, "archive edit: scoped rescan enqueue failed");
    }

    Ok(())
}

pub struct EditResult {
    pub library_id: Uuid,
    pub series_id: Uuid,
    /// The archive path *after* the edit. Same as the source for CBZ/CBT;
    /// the new `.cbz` path for a converted CBR.
    pub archive_path: PathBuf,
    pub page_count_before: usize,
    pub page_count_after: usize,
    pub backup_path: Option<PathBuf>,
    /// Set when the edit changed the file path (CBR→CBZ conversion), so
    /// the caller can update `issue.file_path`. `None` for in-place edits.
    pub moved_to: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("issue {0} not found")]
    IssueGone(String),
    #[error("library {0} writeback disabled (allow_archive_writeback=false)")]
    WritebackDisabled(Uuid),
    #[error("unsupported archive format for editing (CBZ/CBT/CBR only)")]
    UnsupportedFormat,
    #[error("page ops invalid: {0}")]
    Ops(#[from] OpError),
    #[error("upload {0} not found or unreadable")]
    UploadMissing(Uuid),
    #[error("image decode/encode: {0}")]
    Image(String),
    #[error("rewrite: {0}")]
    Rewrite(#[from] RewriteError),
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("archive: {0}")]
    Archive(#[from] ArchiveError),
}

/// Core edit path. Caller holds the per-issue rewrite mutex. Dispatches
/// on the source format: CBZ/CBT rewrite in place; CBR converts to a
/// sibling `.cbz` (RAR has no writer) and reports the new path via
/// [`EditResult::moved_to`].
pub async fn edit_one_issue(
    state: &AppState,
    job: &ArchiveEditJob,
) -> Result<EditResult, EditError> {
    let Some(row) = issue::Entity::find_by_id(&job.issue_id)
        .one(&state.db)
        .await?
    else {
        return Err(EditError::IssueGone(job.issue_id.clone()));
    };
    let lib = entity::library::Entity::find_by_id(row.library_id)
        .one(&state.db)
        .await?
        .ok_or_else(|| EditError::IssueGone(format!("library missing for {}", row.id)))?;
    if !lib.allow_archive_writeback {
        return Err(EditError::WritebackDisabled(lib.id));
    }

    let archive_path = PathBuf::from(&row.file_path);
    let Some(format) = EditFormat::from_path(&archive_path) else {
        return Err(EditError::UnsupportedFormat);
    };
    let cfg = state.cfg();
    let limits = cfg.archive_limits();
    let uploads_dir = cfg.data_path.join("uploads");
    let retain_count = lib.archive_backup_retain_count;
    let jpeg_quality = lib.archive_writeback_jpeg_quality.clamp(1, 100) as u8;
    let ops = job.ops.clone();
    let src_path = archive_path.clone();
    // CBR converts to a sibling `.cbz`; CBZ/CBT rewrite in place.
    let dst_path = if format == EditFormat::Cbr {
        src_path.with_extension("cbz")
    } else {
        src_path.clone()
    };
    let dst_for_closure = dst_path.clone();

    let (backup, before, after) = tokio::task::spawn_blocking(
        move || -> Result<(Option<PathBuf>, usize, usize), EditError> {
            let mut before = 0usize;
            let mut after = 0usize;
            let backup = match format {
                EditFormat::Cbz => {
                    let outcome =
                        archive_rewrite::rewrite_atomic(&src_path, retain_count, |tmp| {
                            let mut src =
                                Cbz::open(&src_path, limits).map_err(RewriteError::ArchiveErr)?;
                            let page_meta = page_meta(&src);
                            before = page_meta.len();
                            let work = apply_ops(&page_meta, &ops, &uploads_dir)
                                .map_err(rewrite_from_edit)?;
                            // CBZ keeps the deflate streams of untouched pages.
                            let output = output_pages_cbz(work, &mut src, jpeg_quality)
                                .map_err(rewrite_from_edit)?;
                            after = output.len();
                            let extras =
                                read_preserved_sidecars(&mut src).map_err(rewrite_from_edit)?;
                            cbz_write::rebuild_pages(&mut src, output, extras, tmp, limits)
                                .map_err(RewriteError::ArchiveErr)?;
                            Ok(())
                        })?;
                    outcome.backup
                }
                EditFormat::Cbt => {
                    let outcome =
                        archive_rewrite::rewrite_atomic(&src_path, retain_count, |tmp| {
                            let mut src =
                                Cbt::open(&src_path, limits).map_err(RewriteError::ArchiveErr)?;
                            let page_meta = page_meta(&src);
                            before = page_meta.len();
                            let work = apply_ops(&page_meta, &ops, &uploads_dir)
                                .map_err(rewrite_from_edit)?;
                            let mats = materialize_pages(work, &mut src, jpeg_quality)
                                .map_err(rewrite_from_edit)?;
                            after = mats.len();
                            let extras =
                                read_preserved_sidecars(&mut src).map_err(rewrite_from_edit)?;
                            cbt_write::write_pages(mats, extras, tmp, limits)
                                .map_err(RewriteError::ArchiveErr)?;
                            Ok(())
                        })?;
                    outcome.backup
                }
                EditFormat::Cbr => {
                    let outcome =
                        archive_rewrite::convert_atomic(&src_path, &dst_for_closure, |tmp| {
                            let mut src =
                                Cbr::open(&src_path, limits).map_err(RewriteError::ArchiveErr)?;
                            let page_meta = page_meta(&src);
                            before = page_meta.len();
                            let work = apply_ops(&page_meta, &ops, &uploads_dir)
                                .map_err(rewrite_from_edit)?;
                            // RAR payloads can't stream-copy into a zip;
                            // every page is materialized + stored (level 0).
                            let mats = materialize_pages(work, &mut src, jpeg_quality)
                                .map_err(rewrite_from_edit)?;
                            after = mats.len();
                            let extras =
                                read_preserved_sidecars(&mut src).map_err(rewrite_from_edit)?;
                            cbz_write::write_pages(mats, extras, tmp, limits)
                                .map_err(RewriteError::ArchiveErr)?;
                            Ok(())
                        })?;
                    outcome.backup
                }
            };
            Ok((backup, before, after))
        },
    )
    .await
    .map_err(|e| EditError::Image(format!("join: {e}")))??;

    state.zip_lru.invalidate(&row.id);

    let moved = (dst_path != archive_path).then(|| dst_path.clone());

    // Bookkeeping: clear thumbnail stamps so the post-scan pipeline
    // regenerates them, stamp the edit, reset the cover index when the
    // page set/order changed, and repoint `file_path` on a CBR conversion.
    let structural = job
        .ops
        .iter()
        .any(|o| matches!(o, PageOp::Remove { .. } | PageOp::Reorder { .. }));
    let mut am = issue::ActiveModel {
        id: Set(row.id.clone()),
        last_rewrite_at: Set(Some(Utc::now().fixed_offset())),
        last_rewrite_kind: Set(Some("edit".to_owned())),
        thumbnails_generated_at: Set(None),
        thumbnail_version: Set(0),
        thumbnails_error: Set(None),
        updated_at: Set(Utc::now().fixed_offset()),
        ..Default::default()
    };
    if structural || moved.is_some() {
        am.cover_page_index = Set(0);
    }
    if let Some(ref new_path) = moved {
        am.file_path = Set(new_path.to_string_lossy().into_owned());
    }
    am.update(&state.db).await?;

    // First CBR conversion in a library stamps `cbr_convert_confirmed_at`
    // so the UI stops prompting for the format change on later edits.
    if format == EditFormat::Cbr && lib.cbr_convert_confirmed_at.is_none() {
        let lib_am = entity::library::ActiveModel {
            id: Set(lib.id),
            cbr_convert_confirmed_at: Set(Some(Utc::now().fixed_offset())),
            updated_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        };
        if let Err(e) = lib_am.update(&state.db).await {
            tracing::warn!(library_id = %lib.id, error = %e, "archive edit: cbr_convert_confirmed_at stamp failed");
        }
    }

    Ok(EditResult {
        library_id: row.library_id,
        series_id: row.series_id,
        archive_path: dst_path,
        page_count_before: before,
        page_count_after: after,
        backup_path: backup,
        moved_to: moved,
    })
}

/// Map an [`OpError`]/[`EditError`] raised inside the rewrite closure back
/// into the closure's `RewriteError` channel. We smuggle the detail
/// through `RewriteError::ArchiveErr(Malformed(..))` so the atomic-swap
/// helper aborts before touching the original file; the outer `?` then
/// surfaces it. (The closure signature is fixed by `rewrite_atomic`.)
fn rewrite_from_edit(e: EditError) -> RewriteError {
    RewriteError::ArchiveErr(ArchiveError::Malformed(e.to_string()))
}

/// Owned (raw-index, name) list of the source's pages in natural-sort
/// order. Taken eagerly so the immutable borrow on `src` ends before the
/// `&mut src` reads in the emit step.
fn page_meta(src: &dyn ComicArchive) -> Vec<(usize, String)> {
    src.pages()
        .iter()
        .map(|e| (e.index, e.name.clone()))
        .collect()
}

/// One page in the editor's working list, after the sequential ops have
/// been applied. `rotation` is the net clockwise rotation; `replacement`
/// holds staged-upload bytes when the page was replaced.
struct Work {
    src_index: usize,
    name: String,
    rotation: u16,
    replacement: Option<Vec<u8>>,
    transforms: Vec<TransformStep>,
}

/// Apply the validated sequential ops to the source page list, producing
/// the final ordered working list. Format-agnostic — the emit step turns
/// this into either `OutputPage`s (CBZ) or materialized bytes (CBT/CBR).
fn apply_ops(
    page_meta: &[(usize, String)],
    ops: &[PageOp],
    uploads_dir: &Path,
) -> Result<Vec<Work>, EditError> {
    simulate_ops(page_meta.len(), ops)?;
    let mut work: Vec<Work> = page_meta
        .iter()
        .map(|(idx, name)| Work {
            src_index: *idx,
            name: name.clone(),
            rotation: 0,
            replacement: None,
            transforms: Vec::new(),
        })
        .collect();

    for op in ops {
        match op {
            PageOp::Remove { ordinal } => {
                work.remove(*ordinal as usize);
            }
            PageOp::Reorder { new_order } => {
                let mut next: Vec<Work> = Vec::with_capacity(new_order.len());
                // `work[i]` can't be cloned (Vec<u8> replacement), so take
                // by swapping out into Options indexed by position.
                let mut slots: Vec<Option<Work>> = work.into_iter().map(Some).collect();
                for &i in new_order {
                    next.push(
                        slots[i as usize]
                            .take()
                            .expect("permutation validated by simulate_ops"),
                    );
                }
                work = next;
            }
            PageOp::Rotate { ordinal, degrees } => {
                let w = &mut work[*ordinal as usize];
                w.rotation = (w.rotation + degrees.degrees()) % 360;
            }
            PageOp::Replace { ordinal, image_id } => {
                let path = uploads_dir.join(image_id.to_string());
                let bytes =
                    std::fs::read(&path).map_err(|_| EditError::UploadMissing(*image_id))?;
                work[*ordinal as usize].replacement = Some(bytes);
            }
            PageOp::Transform { ordinal, chain } => {
                work[*ordinal as usize]
                    .transforms
                    .extend(chain.iter().cloned());
            }
        }
    }
    Ok(work)
}

/// Emit CBZ output pages — untouched pages stream-copy their compressed
/// payload (`Keep`); rotated / replaced / transformed pages re-encode.
fn output_pages_cbz(
    work: Vec<Work>,
    src: &mut Cbz,
    jpeg_quality: u8,
) -> Result<Vec<OutputPage>, EditError> {
    let mut out = Vec::with_capacity(work.len());
    for w in work {
        if !needs_encode(&w) {
            out.push(OutputPage {
                ext: ext_of(&w.name),
                bytes: PageBytes::Keep {
                    src_index: w.src_index,
                },
            });
            continue;
        }
        let src_bytes = match w.replacement {
            Some(bytes) => bytes,
            None => src.read_entry_bytes_by_name(&w.name)?,
        };
        let (encoded, ext) =
            transform_image(&src_bytes, w.rotation % 360, &w.transforms, jpeg_quality)?;
        out.push(OutputPage {
            ext,
            bytes: PageBytes::Encoded {
                bytes: encoded,
                level: 0,
            },
        });
    }
    Ok(out)
}

/// Emit fully-materialized pages `(ext, bytes, level)` for the CBT/CBR
/// writers (no stream-copy: tar is uncompressed and RAR can't copy into a
/// zip). Untouched pages are read + stored (level 0); rotated / replaced /
/// transformed pages re-encode.
fn materialize_pages(
    work: Vec<Work>,
    src: &mut dyn ComicArchive,
    jpeg_quality: u8,
) -> Result<Vec<(String, Vec<u8>, i64)>, EditError> {
    let mut out = Vec::with_capacity(work.len());
    for w in work {
        if !needs_encode(&w) {
            let bytes = src.read_entry_bytes(&w.name)?;
            out.push((ext_of(&w.name), bytes, 0));
            continue;
        }
        let src_bytes = match w.replacement {
            Some(bytes) => bytes,
            None => src.read_entry_bytes(&w.name)?,
        };
        let (encoded, ext) =
            transform_image(&src_bytes, w.rotation % 360, &w.transforms, jpeg_quality)?;
        out.push((ext, encoded, 0));
    }
    Ok(out)
}

/// A page needs decode + re-encode when it was replaced, rotated, or has
/// an image-transform chain. Untouched pages stream-copy / read verbatim.
fn needs_encode(w: &Work) -> bool {
    w.replacement.is_some() || !w.rotation.is_multiple_of(360) || !w.transforms.is_empty()
}

/// Read `ComicInfo.xml` / `MetronInfo.xml` from the source so a page edit
/// preserves them verbatim. Other non-page entries (Thumbs.db, dotfiles)
/// are intentionally dropped, mirroring the sidecar-rewrite path. Generic
/// over the reader so CBZ/CBT/CBR all preserve sidecars on edit.
fn read_preserved_sidecars(
    src: &mut dyn ComicArchive,
) -> Result<Vec<(String, Vec<u8>, i64)>, EditError> {
    let mut extras = Vec::new();
    for name in ["ComicInfo.xml", "MetronInfo.xml"] {
        if src.find(name).is_some() {
            let bytes = src.read_entry_bytes(name)?;
            extras.push((name.to_string(), bytes, 6));
        }
    }
    Ok(extras)
}

/// Lowercase extension (no dot) of an entry name, defaulting to `jpg`.
fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.len() <= 5)
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "jpg".to_owned())
}

/// Decode `bytes`, rotate `rot` degrees clockwise, apply the image
/// transform `chain`, then re-encode. JPEG sources re-encode to JPEG at
/// `quality`; everything else (PNG/WebP/…) re-encodes losslessly to PNG
/// (the `image` crate has no WebP encoder). Returns the encoded bytes +
/// the output extension.
fn transform_image(
    bytes: &[u8],
    rot: u16,
    chain: &[TransformStep],
    quality: u8,
) -> Result<(Vec<u8>, String), EditError> {
    use image::ImageEncoder;
    let fmt = image::guess_format(bytes).map_err(|e| EditError::Image(e.to_string()))?;
    let img = image::load_from_memory(bytes).map_err(|e| EditError::Image(e.to_string()))?;
    let rotated = match rot {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img,
    };
    let rotated = apply_chain(rotated, chain);

    if fmt == image::ImageFormat::Jpeg {
        let rgb = rotated.to_rgb8();
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| EditError::Image(e.to_string()))?;
        Ok((buf, "jpg".to_owned()))
    } else {
        let mut buf = std::io::Cursor::new(Vec::new());
        rotated
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| EditError::Image(e.to_string()))?;
        Ok((buf.into_inner(), "png".to_owned()))
    }
}

async fn enqueue_scoped_rescan(
    state: &AppState,
    library_id: &Uuid,
    series_id: &Uuid,
    issue_id: &str,
) -> anyhow::Result<()> {
    use crate::jobs::scan_series;
    state
        .jobs
        .coalesce_scoped_scan(
            *library_id,
            *series_id,
            None,
            scan_series::JobKind::Issue,
            Some(issue_id.to_owned()),
            true, // force — the file's bytes changed
        )
        .await?;
    Ok(())
}

async fn audit_edit(
    state: &AppState,
    job: &ArchiveEditJob,
    outcome: &Result<EditResult, EditError>,
) {
    let payload = match outcome {
        Ok(r) => serde_json::json!({
            "issue_id": job.issue_id,
            "archive_path": r.archive_path.to_string_lossy(),
            "backup_path": r.backup_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "page_count_before": r.page_count_before,
            "page_count_after": r.page_count_after,
            "ops": job.ops,
        }),
        Err(e) => serde_json::json!({
            "issue_id": job.issue_id,
            "error": e.to_string(),
            "ops": job.ops,
        }),
    };

    let Some(actor_id) = job.actor_id else {
        tracing::info!(issue_id = %job.issue_id, ?payload, "archive edit: anonymous run; no audit row");
        return;
    };

    audit::record(
        &state.db,
        AuditEntry {
            actor_id,
            action: "admin.issue.archive_edit",
            target_type: Some("issue"),
            target_id: Some(job.issue_id.clone()),
            payload,
            ip: job.actor_ip.clone(),
            user_agent: job.actor_ua.clone(),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulate_remove_then_reorder() {
        let ops = vec![
            PageOp::Remove { ordinal: 1 },
            PageOp::Reorder {
                new_order: vec![2, 0, 1],
            },
        ];
        // 4 pages → remove 1 → 3 pages → reorder permutation of 0..3.
        assert_eq!(simulate_ops(4, &ops).unwrap(), 3);
    }

    #[test]
    fn simulate_rejects_out_of_range() {
        let ops = vec![PageOp::Remove { ordinal: 5 }];
        assert!(matches!(
            simulate_ops(3, &ops),
            Err(OpError::OrdinalOutOfRange { .. })
        ));
    }

    #[test]
    fn simulate_rejects_bad_permutation() {
        let ops = vec![PageOp::Reorder {
            new_order: vec![0, 0, 1],
        }];
        assert!(matches!(
            simulate_ops(3, &ops),
            Err(OpError::BadPermutation { .. })
        ));
    }

    #[test]
    fn simulate_rejects_empty_result() {
        let ops = vec![PageOp::Remove { ordinal: 0 }, PageOp::Remove { ordinal: 0 }];
        assert!(matches!(simulate_ops(2, &ops), Err(OpError::EmptyResult)));
    }

    #[test]
    fn ext_of_handles_common_cases() {
        assert_eq!(ext_of("p001.JPG"), "jpg");
        assert_eq!(ext_of("foo/bar.png"), "png");
        assert_eq!(ext_of("noext"), "jpg");
    }
}
