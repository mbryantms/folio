//! Perceptual image hashing for cover-art matching.
//!
//! Three complementary 64-bit hashes computed on every cover (both
//! local-archive covers extracted by the scanner and provider covers
//! written by Apply jobs):
//!
//! - **ahash** — Average-hash. Downscale to 8×8 grayscale, set bit to
//!   1 when the pixel is brighter than the image mean. Cheap. Robust
//!   to color shifts; weak against re-cropping.
//! - **dhash** — Difference-hash. Downscale to 9×8 grayscale, set bit
//!   when pixel[x] < pixel[x+1] across each row. Cheap. Better than
//!   ahash on contrast variations; weak against horizontal flips.
//! - **phash** — Perceptual hash (DCT-II). Downscale to 32×32
//!   grayscale, run a 32×32 DCT, take the top-left 8×8 (lowest
//!   frequency band), bit-set on > median. Robust to JPEG re-encode,
//!   resize, gamma shifts. The workhorse — pairs well with the other
//!   two as cross-validators.
//!
//! All three return `i64` (re-interpretation of the 64-bit pattern).
//! Postgres stores them in `issue_cover.{phash,dhash,ahash}` as
//! signed 8-byte ints — there's no native unsigned int type but the
//! Hamming-distance compare just XORs the bit patterns so the sign
//! doesn't matter.
//!
//! Hamming distance is the number of differing bits between two
//! hashes. Typical cover-match thresholds (matcher.rs uses these):
//!
//! - distance ≤ 6 (out of 64) — essentially the same image, very
//!   high confidence even across re-encodes.
//! - 7-12 — same image with substantive editing (border, watermark,
//!   re-color). Boost.
//! - 13-20 — visually similar but probably a different release.
//!   Mild boost.
//! - >20 — different image. No boost.
//!
//! [`similarity_score`] folds distance into a 0..=1 score the
//! matcher can multiply into its weighted sum.
//!
//! metadata-providers-1.0 M9.

use archive::ArchiveLimits;
use entity::{issue, issue_cover};
use image::DynamicImage;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect, Set,
};

const HASH_SIDE: u32 = 8;
const DHASH_WIDTH: u32 = 9;
const PHASH_SIDE: u32 = 32;
const PHASH_SAMPLE: usize = 8;

/// 64-bit average hash. Lower bits are leftmost pixel of the top
/// row, marching left-to-right then top-to-bottom — keep the layout
/// in sync with `dhash` / `phash` since they all feed into the same
/// XOR comparator.
pub fn ahash(img: &DynamicImage) -> i64 {
    let gray = img
        .resize_exact(HASH_SIDE, HASH_SIDE, image::imageops::FilterType::Triangle)
        .to_luma8();
    let pixels: Vec<u8> = gray.as_raw().clone();
    let sum: u32 = pixels.iter().map(|p| *p as u32).sum();
    let mean = (sum / pixels.len() as u32) as u8;
    let mut bits: u64 = 0;
    for (i, p) in pixels.iter().enumerate() {
        if *p > mean {
            bits |= 1u64 << i;
        }
    }
    bits as i64
}

/// 64-bit difference hash. Resize to 9×8 grayscale and set bit when
/// the pixel to the right is brighter — yields 8 bits per row × 8
/// rows = 64.
pub fn dhash(img: &DynamicImage) -> i64 {
    let gray = img
        .resize_exact(
            DHASH_WIDTH,
            HASH_SIDE,
            image::imageops::FilterType::Triangle,
        )
        .to_luma8();
    let mut bits: u64 = 0;
    let mut idx = 0usize;
    for y in 0..HASH_SIDE {
        for x in 0..HASH_SIDE {
            let left = gray.get_pixel(x, y).0[0];
            let right = gray.get_pixel(x + 1, y).0[0];
            if left < right {
                bits |= 1u64 << idx;
            }
            idx += 1;
        }
    }
    bits as i64
}

/// 64-bit perceptual hash. Downscale to 32×32, DCT, take the 8×8
/// low-frequency block (skipping the DC term), bit-set on > median.
pub fn phash(img: &DynamicImage) -> i64 {
    let gray = img
        .resize_exact(
            PHASH_SIDE,
            PHASH_SIDE,
            image::imageops::FilterType::Triangle,
        )
        .to_luma8();
    // Materialize the 32×32 pixel grid as f32 for the DCT pass.
    let mut grid = [[0.0f32; PHASH_SIDE as usize]; PHASH_SIDE as usize];
    for y in 0..PHASH_SIDE {
        for x in 0..PHASH_SIDE {
            grid[y as usize][x as usize] = gray.get_pixel(x, y).0[0] as f32;
        }
    }
    let dct = dct2_2d(&grid);
    // Pick the 8×8 low-frequency block (top-left), then drop the DC
    // term ([0][0]) to avoid biasing the median on overall brightness.
    let mut samples = Vec::with_capacity(PHASH_SAMPLE * PHASH_SAMPLE - 1);
    for (y, row) in dct.iter().enumerate().take(PHASH_SAMPLE) {
        for (x, v) in row.iter().enumerate().take(PHASH_SAMPLE) {
            if x == 0 && y == 0 {
                continue;
            }
            samples.push(*v);
        }
    }
    // Median (not mean) — robust to a single outlier DCT coefficient.
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let mut bits: u64 = 0;
    for (i, v) in samples.iter().enumerate() {
        if *v > median {
            bits |= 1u64 << i;
        }
    }
    bits as i64
}

/// Compute all three hashes in one pass — saves the redundant resize
/// when the caller wants the full set (which is essentially every
/// production call site).
pub fn all_hashes(img: &DynamicImage) -> (i64, i64, i64) {
    (phash(img), dhash(img), ahash(img))
}

/// Hamming distance between two 64-bit hash patterns. Returns
/// 0..=64.
pub fn hamming_distance(a: i64, b: i64) -> u32 {
    (a ^ b).count_ones()
}

/// Fold distance into a 0..=1.0 similarity score. Anything past
/// `threshold` snaps to 0 so the matcher doesn't credit
/// merely-not-completely-different covers. Inside the threshold,
/// linearly decreases from 1 (perfect) to 0 (at threshold).
///
/// Default `threshold = 20` works well for cover-art across CV /
/// Metron variants; tighter values (e.g. 8) prefer only essentially-
/// identical images.
pub fn similarity_score(distance: u32, threshold: u32) -> f32 {
    if distance >= threshold {
        return 0.0;
    }
    1.0 - (distance as f32 / threshold as f32)
}

// ───────── DB integration ─────────

/// Upsert the archive-extracted primary cover for `issue_id` with
/// fresh hashes. Idempotent: when a row already exists at
/// `(issue_id, kind='primary', ordinal=0, source_provider='archive_extracted')`
/// the hash columns are overwritten in place. Other primary rows
/// (e.g. a CV-applied cover) are left untouched — multiple primary
/// rows can co-exist with `is_active` distinguishing the one served
/// by the cover-URL endpoint.
///
/// The local_path is stored relative to `{data_path}` so it's
/// portable across deploys. Callers pass the on-disk cover path; we
/// strip the data_path prefix.
///
/// metadata-providers-1.0 M9.
pub async fn upsert_archive_cover_hashes<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    local_path: &str,
    img: &DynamicImage,
) -> Result<uuid::Uuid, sea_orm::DbErr> {
    let hashes = all_hashes(img);
    let width = img.width() as i32;
    let height = img.height() as i32;
    upsert_archive_cover_hashes_from_parts(db, issue_id, local_path, hashes, width, height).await
}

/// Same as [`upsert_archive_cover_hashes`] but takes pre-computed
/// hash + dimension values. Used by the post-scan hash path that
/// decodes the source archive page once and shares the result
/// between the thumbnail encoder and the hash writer, and by the
/// backfill sweep that decodes archive bytes directly.
///
/// `hashes` is `(phash, dhash, ahash)` in the same order
/// [`all_hashes`] returns.
pub async fn upsert_archive_cover_hashes_from_parts<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    local_path: &str,
    hashes: (i64, i64, i64),
    width: i32,
    height: i32,
) -> Result<uuid::Uuid, sea_orm::DbErr> {
    let (p, d, a) = hashes;
    let existing = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(issue_id))
        .filter(issue_cover::Column::Kind.eq("primary"))
        .filter(issue_cover::Column::Ordinal.eq(0))
        .filter(issue_cover::Column::SourceProvider.eq("archive_extracted"))
        .one(db)
        .await?;
    let now = chrono::Utc::now().fixed_offset();
    if let Some(prev) = existing {
        let id = prev.id;
        let mut am: issue_cover::ActiveModel = prev.into();
        am.phash = Set(Some(p));
        am.dhash = Set(Some(d));
        am.ahash = Set(Some(a));
        am.width = Set(Some(width));
        am.height = Set(Some(height));
        am.local_path = Set(local_path.into());
        am.fetched_at = Set(now);
        am.update(db).await?;
        return Ok(id);
    }
    let id = uuid::Uuid::now_v7();
    issue_cover::ActiveModel {
        id: Set(id),
        issue_id: Set(issue_id.into()),
        kind: Set("primary".into()),
        ordinal: Set(0),
        source_provider: Set(Some("archive_extracted".into())),
        source_external_id: Set(None),
        source_url: Set(None),
        variant_label: Set(None),
        variant_artist_person_id: Set(None),
        local_path: Set(local_path.into()),
        width: Set(Some(width)),
        height: Set(Some(height)),
        phash: Set(Some(p)),
        dhash: Set(Some(d)),
        ahash: Set(Some(a)),
        fetched_at: Set(now),
        // Archive-extracted covers default to inactive — the
        // operator-facing cover slot is owned by the most recently
        // applied provider cover (or the legacy on-disk thumb when
        // none exists). The hash row is a side-channel for the
        // matching engine, not the display surface.
        is_active: Set(false),
    }
    .insert(db)
    .await?;
    Ok(id)
}

/// Open an archive, decode its cover page, and compute all three
/// perceptual hashes — synchronous because it does blocking I/O and
/// CPU-bound image decode. Always call from `spawn_blocking`.
///
/// Hashing the source archive page (rather than the on-disk WebP
/// thumbnail) keeps us on the same reference distribution
/// ComicTagger uses, which is what the cover-Hamming ladder
/// constants in `matcher.rs` are calibrated against. Going through
/// the thumbnail introduces extra encode losses on our side that the
/// provider's hosted cover doesn't have, biasing Hamming distances
/// upward by a couple of bits — enough to push genuine matches out
/// of the High bucket.
///
/// Falls back to page 0 when `cover_page_index` is past the end of
/// the archive (defensive — the scanner stamps the column from
/// ComicInfo so a stale value is plausible after a retag).
pub fn compute_archive_cover(
    archive_path: &std::path::Path,
    cover_page_index: usize,
    limits: ArchiveLimits,
) -> Result<ArchiveCoverHashes, ArchiveCoverError> {
    let mut a = archive::open(archive_path, limits)?;
    let entry_name = {
        let pages = a.pages();
        if pages.is_empty() {
            return Err(ArchiveCoverError::NoPages);
        }
        pages
            .get(cover_page_index)
            .copied()
            .or_else(|| pages.first().copied())
            .expect("non-empty pages")
            .name
            .clone()
    };
    let bytes = a.read_entry_bytes(&entry_name)?;
    let img =
        image::load_from_memory(&bytes).map_err(|e| ArchiveCoverError::Decode(e.to_string()))?;
    let hashes = all_hashes(&img);
    Ok(ArchiveCoverHashes {
        hashes,
        width: img.width() as i32,
        height: img.height() as i32,
    })
}

/// Output of [`compute_archive_cover`]. Carries the hash triple plus
/// source dimensions so the caller can write `issue_cover.width` /
/// `height` without a second decode.
#[derive(Debug, Clone, Copy)]
pub struct ArchiveCoverHashes {
    pub hashes: (i64, i64, i64),
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveCoverError {
    #[error("archive open/read: {0}")]
    Archive(#[from] archive::ArchiveError),
    #[error("image decode: {0}")]
    Decode(String),
    #[error("archive has no pages")]
    NoPages,
}

/// Backfill helper — opens the parent issue's archive, decodes its
/// cover page, computes hashes, writes back to
/// `issue_cover.{phash,dhash,ahash,width,height}` in place. Used by
/// both the admin-triggered backfill endpoint and the startup drain
/// for rows that pre-date the inline-hash path.
///
/// Returns `Ok(true)` when the row was hashed, `Ok(false)` when the
/// archive was missing or undecodable (soft-skip — the row stays
/// NULL and a future run will retry), `Err(_)` only on DB-write
/// failures.
pub async fn backfill_row<C: ConnectionTrait>(
    db: &C,
    cover_row: &issue_cover::Model,
    issue_file_path: &std::path::Path,
    cover_page_index: usize,
    archive_limits: ArchiveLimits,
) -> Result<bool, std::io::Error> {
    let path = issue_file_path.to_path_buf();
    let hashed = tokio::task::spawn_blocking(move || {
        compute_archive_cover(&path, cover_page_index, archive_limits)
    })
    .await
    .map_err(|e| std::io::Error::other(format!("phash backfill join: {e}")))?;
    let parts = match hashed {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(
                cover_id = %cover_row.id,
                path = %issue_file_path.display(),
                error = %e,
                "phash backfill: archive read/decode failed (soft-skip)"
            );
            return Ok(false);
        }
    };
    let mut am: issue_cover::ActiveModel = cover_row.clone().into();
    let (p, d, a) = parts.hashes;
    am.phash = Set(Some(p));
    am.dhash = Set(Some(d));
    am.ahash = Set(Some(a));
    am.width = Set(Some(parts.width));
    am.height = Set(Some(parts.height));
    if let Err(e) = am.update(db).await {
        return Err(std::io::Error::other(format!("phash backfill update: {e}")));
    }
    Ok(true)
}

/// Look up the representative perceptual hash for a series — used by
/// M9.5's search-side ranking to compare against candidate covers.
/// Picks the lowest-`ordinal` primary cover with a non-null phash on
/// any active issue in the series, preferring archive-extracted rows
/// (the user's actual file) over provider-applied ones (which could
/// be drift from a prior, possibly-wrong match).
///
/// Returns `None` when no issue in the series has a hashed cover yet
/// — the scorer falls back to text-only matching in that case.
///
/// metadata-providers-1.0 M9.5.
pub async fn series_representative_phash<C: ConnectionTrait>(
    db: &C,
    series_id: uuid::Uuid,
) -> Result<Option<i64>, sea_orm::DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
    #[derive(FromQueryResult)]
    struct Row {
        phash: i64,
    }
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        // Join issue → issue_cover; prefer archive_extracted (sort
        // key 0) over other sources (1). Within each tier, prefer
        // the earliest issue (created_at ASC) so series with a long
        // run still pick a stable representative.
        r"SELECT ic.phash
          FROM issues i
          JOIN issue_cover ic
            ON ic.issue_id = i.id
           AND ic.kind = 'primary'
           AND ic.ordinal = 0
           AND ic.phash IS NOT NULL
          WHERE i.series_id = $1
            AND i.state = 'active'
            AND i.removed_at IS NULL
          ORDER BY
            CASE WHEN ic.source_provider = 'archive_extracted' THEN 0 ELSE 1 END,
            i.created_at ASC
          LIMIT 1",
        [series_id.into()],
    );
    Ok(Row::find_by_statement(stmt).one(db).await?.map(|r| r.phash))
}

/// Look up the perceptual hash for a single issue's primary cover.
/// Same archive-extracted preference as
/// [`series_representative_phash`].
///
/// metadata-providers-1.0 M9.5.
pub async fn issue_phash<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> Result<Option<i64>, sea_orm::DbErr> {
    use sea_orm::{DatabaseBackend, FromQueryResult, Statement};
    #[derive(FromQueryResult)]
    struct Row {
        phash: i64,
    }
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r"SELECT ic.phash
          FROM issue_cover ic
          WHERE ic.issue_id = $1
            AND ic.kind = 'primary'
            AND ic.ordinal = 0
            AND ic.phash IS NOT NULL
          ORDER BY
            CASE WHEN ic.source_provider = 'archive_extracted' THEN 0 ELSE 1 END
          LIMIT 1",
        [issue_id.into()],
    );
    Ok(Row::find_by_statement(stmt).one(db).await?.map(|r| r.phash))
}

/// Fetch + decode + hash a remote cover image. Failure soft-returns
/// `None` so a slow CDN / decode error never blocks the search-side
/// scoring. Bounded by an aggressive timeout — covers are tiny and a
/// search shouldn't stall waiting on one.
///
/// metadata-providers-1.0 M9.5.
pub async fn fetch_and_hash_cover(
    client: &reqwest::Client,
    url: &str,
    timeout: std::time::Duration,
) -> Option<i64> {
    let res = match tokio::time::timeout(timeout, client.get(url).send()).await {
        Ok(Ok(r)) if r.status().is_success() => r,
        Ok(Ok(r)) => {
            tracing::debug!(url, status = %r.status(), "phash fetch: non-2xx");
            return None;
        }
        Ok(Err(e)) => {
            tracing::debug!(url, error = %e, "phash fetch: transport error");
            return None;
        }
        Err(_) => {
            tracing::debug!(url, "phash fetch: timeout");
            return None;
        }
    };
    let bytes = match tokio::time::timeout(timeout, res.bytes()).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            tracing::debug!(url, error = %e, "phash fetch: body read failed");
            return None;
        }
        Err(_) => {
            tracing::debug!(url, "phash fetch: body timeout");
            return None;
        }
    };
    // Decoding can be CPU-intensive on large covers; punt to a
    // blocking task so the async runtime stays free.
    let bytes_for_blocking = bytes.to_vec();
    let img = tokio::task::spawn_blocking(move || image::load_from_memory(&bytes_for_blocking))
        .await
        .ok()?
        .ok()?;
    Some(phash(&img))
}

/// Outcome of a phash backfill sweep — exposed via the admin
/// endpoint so the operator can see how many rows landed in each
/// category.
#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct BackfillOutcome {
    /// Rows visited (had `phash IS NULL` at the start of the sweep).
    pub considered: usize,
    /// Rows whose hashes wrote successfully.
    pub hashed: usize,
    /// Rows skipped because the file was missing or undecodable.
    pub skipped: usize,
    /// Rows that errored during DB update.
    pub errored: usize,
}

/// Bounded so a single admin click can't tie up the request handler.
pub const BACKFILL_BATCH_CAP: usize = 500;

/// Walk every archive-extracted `issue_cover` row with NULL phash,
/// open the parent issue's archive, decode its cover page, and
/// write the hashes back. Bounded by [`BACKFILL_BATCH_CAP`].
/// Operators re-click to drain larger backlogs; the startup drain
/// (see `app::serve`) loops until empty.
///
/// Only `source_provider = 'archive_extracted'` rows are touched.
/// Provider covers (CV / Metron) are hashed inline at apply time;
/// a NULL phash on a provider row means decode failed during the
/// apply itself, which the backfill can't recover from.
pub async fn run_backfill<C: ConnectionTrait>(
    db: &C,
    archive_limits: ArchiveLimits,
) -> Result<BackfillOutcome, sea_orm::DbErr> {
    let rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::Phash.is_null())
        .filter(issue_cover::Column::SourceProvider.eq("archive_extracted"))
        .find_also_related(issue::Entity)
        .limit(BACKFILL_BATCH_CAP as u64)
        .all(db)
        .await?;
    let considered = rows.len();
    let mut hashed = 0usize;
    let mut skipped = 0usize;
    let mut errored = 0usize;
    for (cover, issue) in rows {
        let Some(issue) = issue else {
            // Orphan cover row — parent issue gone. Soft-skip; an
            // operator can clean these up via the orphan sweep.
            tracing::debug!(cover_id = %cover.id, "phash backfill: orphan cover row, parent issue missing");
            skipped += 1;
            continue;
        };
        let cover_idx = usize::try_from(issue.cover_page_index.max(0)).unwrap_or(0);
        let path = std::path::Path::new(&issue.file_path);
        match backfill_row(db, &cover, path, cover_idx, archive_limits).await {
            Ok(true) => hashed += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                tracing::warn!(cover_id = %cover.id, error = %e, "phash backfill: db update failed");
                errored += 1;
            }
        }
    }
    Ok(BackfillOutcome {
        considered,
        hashed,
        skipped,
        errored,
    })
}

// ───────── 2D DCT-II ─────────

/// Naïve 32×32 DCT-II — small enough that the O(N⁴) cost is fine and
/// pulling in a `rustdct` dep would dwarf the runtime savings.
/// Returns the transformed coefficients; only the top-left block is
/// inspected by [`phash`].
// Clippy's `needless_range_loop` doesn't like the nested arithmetic-
// on-index pattern, but the DCT formula is *defined* over the
// index ranges — rewriting as `.iter().enumerate()` would make the
// math substantially harder to read. The lint isn't load-bearing
// here.
#[allow(clippy::needless_range_loop)]
fn dct2_2d(
    grid: &[[f32; PHASH_SIDE as usize]; PHASH_SIDE as usize],
) -> [[f32; PHASH_SIDE as usize]; PHASH_SIDE as usize] {
    use std::f32::consts::PI;
    let n = PHASH_SIDE as usize;
    let mut row_pass = [[0.0f32; PHASH_SIDE as usize]; PHASH_SIDE as usize];
    // Row-wise 1D DCT-II.
    for y in 0..n {
        for u in 0..n {
            let mut sum = 0.0f32;
            for x in 0..n {
                sum +=
                    grid[y][x] * (PI * (2.0 * x as f32 + 1.0) * u as f32 / (2.0 * n as f32)).cos();
            }
            row_pass[y][u] = sum;
        }
    }
    // Column-wise 1D DCT-II on the row-pass result.
    let mut out = [[0.0f32; PHASH_SIDE as usize]; PHASH_SIDE as usize];
    for u in 0..n {
        for v in 0..n {
            let mut sum = 0.0f32;
            for y in 0..n {
                sum += row_pass[y][u]
                    * (PI * (2.0 * y as f32 + 1.0) * v as f32 / (2.0 * n as f32)).cos();
            }
            out[v][u] = sum;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn solid(color: [u8; 3]) -> DynamicImage {
        let buf = ImageBuffer::from_fn(64, 64, |_, _| Rgb(color));
        DynamicImage::ImageRgb8(buf)
    }

    fn gradient_horizontal() -> DynamicImage {
        let buf = ImageBuffer::from_fn(64, 64, |x, _| Rgb([x as u8 * 4, x as u8 * 4, x as u8 * 4]));
        DynamicImage::ImageRgb8(buf)
    }

    fn gradient_vertical() -> DynamicImage {
        let buf = ImageBuffer::from_fn(64, 64, |_, y| Rgb([y as u8 * 4, y as u8 * 4, y as u8 * 4]));
        DynamicImage::ImageRgb8(buf)
    }

    #[test]
    fn identical_images_have_zero_distance() {
        let a = gradient_horizontal();
        let b = gradient_horizontal();
        assert_eq!(hamming_distance(phash(&a), phash(&b)), 0);
        assert_eq!(hamming_distance(dhash(&a), dhash(&b)), 0);
        assert_eq!(hamming_distance(ahash(&a), ahash(&b)), 0);
    }

    #[test]
    fn very_different_images_have_large_distance() {
        // A horizontal gradient and a vertical gradient share the
        // same mean brightness but differ in DCT energy distribution.
        // phash should pick that up; dhash definitely does (rows
        // alternate light/dark order). ahash is the weakest of the
        // three — its sensitivity is purely above/below-mean per
        // pixel, so for two same-mean images it's coincidence; not
        // gated here.
        let a = gradient_horizontal();
        let b = gradient_vertical();
        assert!(
            hamming_distance(phash(&a), phash(&b)) > 8,
            "phash should distinguish horizontal vs vertical gradient"
        );
        assert!(
            hamming_distance(dhash(&a), dhash(&b)) > 8,
            "dhash should distinguish horizontal vs vertical gradient"
        );
    }

    #[test]
    fn jpeg_recompression_keeps_phash_close() {
        // Re-encode the same gradient as JPEG, decode, hash. phash
        // should land near zero distance — the whole point of the
        // DCT-based approach is JPEG tolerance.
        let original = gradient_horizontal();
        let mut buf = std::io::Cursor::new(Vec::new());
        original
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .unwrap();
        let recoded = image::load_from_memory(buf.get_ref()).unwrap();
        let d = hamming_distance(phash(&original), phash(&recoded));
        assert!(d <= 4, "phash should be JPEG-tolerant; got distance {d}");
    }

    #[test]
    fn similarity_score_clamps_at_threshold() {
        assert_eq!(similarity_score(0, 20), 1.0);
        assert!(similarity_score(10, 20) > 0.49 && similarity_score(10, 20) < 0.51);
        assert_eq!(similarity_score(20, 20), 0.0);
        assert_eq!(similarity_score(25, 20), 0.0);
    }

    #[test]
    fn all_hashes_returns_tuple_in_phash_dhash_ahash_order() {
        let img = gradient_horizontal();
        let (p, d, a) = all_hashes(&img);
        assert_eq!(p, phash(&img));
        assert_eq!(d, dhash(&img));
        assert_eq!(a, ahash(&img));
    }

    #[test]
    fn solid_colors_dont_panic_on_zero_variance() {
        // phash median calculation has been known to choke on
        // constant input (all DCT coefficients near 0). Sanity-check
        // the path doesn't panic + returns *some* deterministic
        // hash.
        let img = solid([128, 128, 128]);
        let _ = phash(&img);
        let _ = dhash(&img);
        let _ = ahash(&img);
    }
}
