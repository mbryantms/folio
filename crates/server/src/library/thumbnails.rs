//! Cover + per-page thumbnail generation.
//!
//! Two variants:
//!   - **`Cover`** — page 0 only, large (`COVER_MAX_WIDTH`), Lanczos3 + q=80.
//!     Drives library / series / issue cover grids.
//!   - **`Strip`** — every page, small (`STRIP_MAX_WIDTH`), Triangle + q=50.
//!     Drives the reader page-strip overlay.
//!
//! Versioning: a single [`THUMBNAIL_VERSION`] constant lets us evolve the
//! pipeline (size / filter / quality) without ad-hoc migrations. The post-scan
//! cover worker stamps each successful issue with the current value; the
//! catchup sweep finds rows with `thumbnail_version < CURRENT` and re-enqueues.
//! Bump the constant when you change a generation parameter (note: format
//! choice is per-library now, not part of the version).
//!
//! Format is per-library (`libraries.thumbnail_format`): one of `webp` (the
//! default), `jpeg`, or `png`. The on-disk extension matches the format.
//! Reads are format-agnostic — `find_existing_*` probes all known
//! extensions, so a library mid-format-change keeps serving its old thumbs
//! until the admin force-recreates.
//!
//! Path scheme on disk (under `data_dir/thumbs/`):
//!   - Cover (page 0)  → `<issue_id>.<ext>`
//!     (kept at the top level for backwards compat with the cover-URL
//!     builders in series.rs / issues.rs — don't move)
//!   - Strip page N     → `<issue_id>/s/<n>.<ext>`
//!
//! `generate_*` functions take a borrowed archive handle so callers can hold
//! a cached CBZ handle or let post-scan workers use any scanner-supported
//! archive format without duplicating thumbnail logic.

use archive::{ArchiveEntry, ArchiveError, ComicArchive};
use fast_image_resize::{
    FilterType as FirFilter, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image as FirImage, ImageRef as FirImageRef},
};
use image::{
    DynamicImage, GenericImageView, ImageEncoder,
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType as PngFilterType, PngEncoder},
    },
};
use rayon::prelude::*;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Bump when any per-variant constant below changes (or when a generation
/// parameter affecting visual output changes). Format selection is
/// per-library and not part of this version. Reads with `thumbnail_version
/// < CURRENT` get re-enqueued by the catchup sweep.
///
/// v2: WebP encoder switched from `image`'s lossless-only codec to libwebp
/// (`webp` crate) at q=80/q=50. Files shrink 5-10×; visual output changes
/// noticeably (lossless → lossy), which is why the version bumped.
///
/// v3: resize swapped from `image::imageops::resize` (scalar Lanczos3 /
/// Triangle) to `fast_image_resize` (SIMD Convolution). Output bytes
/// differ from v2 at the sub-pixel level even though the visual filter is
/// equivalent (Triangle ≈ Bilinear). Bumping forces a one-time recompute
/// so admins don't end up serving a mix of v2/v3 bytes for the same issue.
///
/// v4: `STRIP_MAX_WIDTH` raised from 160 → 400 px. The reader page-strip
/// scales the active thumbnail by 1.6× and renders double-page spreads at
/// `w-48` (192 CSS px), which reaches ~614 device px on a 2× DPR display
/// — far past what a 160 px source can paint without visible blur. 400 px
/// is the smallest source that stays crisp at the active scale on common
/// HiDPI hardware. Bumping forces a one-time strip recompute via the
/// catchup sweep; cover variant is unchanged.
pub const THUMBNAIL_VERSION: i32 = 4;

/// Cover variant — used by the issue / series / library card grids. Wide
/// enough to render at 2× on a typical Retina card without blurring; small
/// enough to keep total disk usage reasonable for large libraries.
pub const COVER_MAX_WIDTH: u32 = 600;
const COVER_FILTER: FirFilter = FirFilter::Lanczos3;
pub const DEFAULT_COVER_QUALITY: u8 = 80;

/// Strip variant — feeds the reader page-strip overlay. Inactive thumbs
/// render at `w-24` (96 CSS px) for singles or `w-48` (192 CSS px) for
/// double-page spreads. The *active* thumb gets `scale-[1.6]`, so a
/// double-page active thumb paints at 192 × 1.6 = 307 CSS px = ~614 device
/// px on a 2× DPR display. 400 px is the smallest source that lands a
/// reasonably-crisp active preview without overshooting into cover-sized
/// disk cost; the file-size delta over the old 160 px cap is roughly 2-3×
/// in WebP at the same quality.
pub const STRIP_MAX_WIDTH: u32 = 400;
/// `image::imageops::FilterType::Triangle` was effectively a bilinear
/// filter; `fast_image_resize::FilterType::Bilinear` is the direct
/// equivalent under the new SIMD path.
const STRIP_FILTER: FirFilter = FirFilter::Bilinear;
pub const DEFAULT_STRIP_QUALITY: u8 = 50;

/// Extensions we recognise on read. Order is the lookup priority — we
/// probe newest-first because most libraries that have *any* file at all
/// will have it in the format their last regen used. Add new formats here
/// (and to [`ThumbFormat`]) when extending support.
const KNOWN_EXTS: &[&str] = &["webp", "jpg", "png"];

/// Variant of thumbnail to generate / serve. New variants slot in without
/// touching call sites — `path_for` and `generate` dispatch on this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Cover,
    Strip,
}

impl Variant {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cover" => Some(Self::Cover),
            "strip" => Some(Self::Strip),
            _ => None,
        }
    }

    fn max_width(self) -> u32 {
        match self {
            Self::Cover => COVER_MAX_WIDTH,
            Self::Strip => STRIP_MAX_WIDTH,
        }
    }
    fn filter(self) -> FirFilter {
        match self {
            Self::Cover => COVER_FILTER,
            Self::Strip => STRIP_FILTER,
        }
    }
    /// Stable string for use in tracing fields (the `FirFilter` enum doesn't
    /// implement Display).
    fn filter_name(self) -> &'static str {
        match self {
            Self::Cover => "lanczos3",
            Self::Strip => "bilinear",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailQuality {
    cover: u8,
    strip: u8,
}

impl ThumbnailQuality {
    pub fn new(cover: i32, strip: i32) -> Self {
        Self {
            cover: cover.clamp(0, 100) as u8,
            strip: strip.clamp(0, 100) as u8,
        }
    }

    fn for_variant(self, variant: Variant) -> u8 {
        match variant {
            Variant::Cover => self.cover,
            Variant::Strip => self.strip,
        }
    }
}

impl Default for ThumbnailQuality {
    fn default() -> Self {
        Self {
            cover: DEFAULT_COVER_QUALITY,
            strip: DEFAULT_STRIP_QUALITY,
        }
    }
}

/// Encode format for a generated thumbnail. Per-library setting; the
/// admin tab on the libraries dashboard exposes the choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThumbFormat {
    #[default]
    Webp,
    Jpeg,
    Png,
}

impl ThumbFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "webp" => Some(Self::Webp),
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            _ => None,
        }
    }

    /// Wire form (db column + API). Always lowercase, no aliases.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
        }
    }

    /// File extension on disk (no leading dot).
    pub fn ext(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }

    /// HTTP `Content-Type` value for this format.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }

    /// Resolve a file's format from its extension (case-insensitive). Used
    /// by the HTTP handler so served bytes carry the right `Content-Type`
    /// regardless of which format the file was encoded as.
    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "webp" => Some(Self::Webp),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ThumbError {
    #[error("archive error: {0}")]
    Archive(#[from] ArchiveError),
    #[error("page index out of range")]
    PageOutOfRange,
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

fn thumbs_root(data_dir: &Path) -> PathBuf {
    data_dir.join("thumbs")
}

/// Disk path the cover thumbnail will be *written* to for the given format.
/// For reads, prefer [`find_existing_cover`] which probes all known
/// extensions in case the library's format setting changed since the last
/// generation.
pub fn cover_path(data_dir: &Path, issue_id: &str, format: ThumbFormat) -> PathBuf {
    thumbs_root(data_dir).join(format!("{issue_id}.{}", format.ext()))
}

/// Disk path the per-page strip thumbnail will be *written* to. Strip
/// thumbs live in a per-issue subdirectory to keep the top-level
/// `thumbs/` dir from ballooning into hundreds of thousands of entries.
pub fn strip_path(
    data_dir: &Path,
    issue_id: &str,
    page_index: usize,
    format: ThumbFormat,
) -> PathBuf {
    thumbs_root(data_dir)
        .join(issue_id)
        .join("s")
        .join(format!("{page_index}.{}", format.ext()))
}

/// Backwards-compat dispatcher. The legacy `thumb_path` API hard-coded
/// "page 0 = cover, anything else = per-page". Used by the inline-fallback
/// HTTP handler to write a freshly-generated thumb in the library's
/// configured format.
pub fn thumb_path(
    data_dir: &Path,
    issue_id: &str,
    page_index: usize,
    format: ThumbFormat,
) -> PathBuf {
    if page_index == 0 {
        cover_path(data_dir, issue_id, format)
    } else {
        strip_path(data_dir, issue_id, page_index, format)
    }
}

/// Disk path for any (variant, page) pair as written by the encoder.
pub fn variant_path(
    data_dir: &Path,
    issue_id: &str,
    variant: Variant,
    page_index: usize,
    format: ThumbFormat,
) -> PathBuf {
    match variant {
        Variant::Cover => cover_path(data_dir, issue_id, format),
        Variant::Strip => strip_path(data_dir, issue_id, page_index, format),
    }
}

/// Per-issue thumbnail directory. Used by cleanup hooks (M5).
pub fn issue_thumbs_dir(data_dir: &Path, issue_id: &str) -> PathBuf {
    thumbs_root(data_dir).join(issue_id)
}

/// Find an existing on-disk cover regardless of format. Probes
/// `<issue_id>.<ext>` for each known extension and returns the first hit.
/// Returns `None` if no cover exists in any format.
pub fn find_existing_cover(data_dir: &Path, issue_id: &str) -> Option<PathBuf> {
    for ext in KNOWN_EXTS {
        let p = thumbs_root(data_dir).join(format!("{issue_id}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Find an existing strip thumb regardless of format.
pub fn find_existing_strip(data_dir: &Path, issue_id: &str, page_index: usize) -> Option<PathBuf> {
    let dir = thumbs_root(data_dir).join(issue_id).join("s");
    for ext in KNOWN_EXTS {
        let p = dir.join(format!("{page_index}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Count existing per-page strip thumbnails for an issue, regardless of
/// format. If multiple formats exist for the same page, it still counts as
/// one generated page.
pub fn count_existing_strips(data_dir: &Path, issue_id: &str) -> Result<usize, std::io::Error> {
    let dir = thumbs_root(data_dir).join(issue_id).join("s");
    if !dir.exists() {
        return Ok(0);
    }
    let mut pages = std::collections::HashSet::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((stem, ext)) = name.rsplit_once('.') else {
            continue;
        };
        if KNOWN_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)) && stem.parse::<usize>().is_ok() {
            pages.insert(stem.to_owned());
        }
    }
    Ok(pages.len())
}

/// Find an existing variant on disk regardless of format.
pub fn find_existing_variant(
    data_dir: &Path,
    issue_id: &str,
    variant: Variant,
    page_index: usize,
) -> Option<PathBuf> {
    match variant {
        Variant::Cover => find_existing_cover(data_dir, issue_id),
        Variant::Strip => find_existing_strip(data_dir, issue_id, page_index),
    }
}

/// Best-effort wipe of an issue's cover thumbnail across every known
/// extension. Leaves the per-issue strip subtree intact — useful for
/// "regenerate cover only" admin flows that shouldn't throw away strip
/// work the user just paid to encode.
pub fn wipe_issue_cover(data_dir: &Path, issue_id: &str) {
    for ext in KNOWN_EXTS {
        let cover = thumbs_root(data_dir).join(format!("{issue_id}.{ext}"));
        if cover.exists()
            && let Err(e) = fs::remove_file(&cover)
        {
            tracing::warn!(path = %cover.display(), error = %e, "wipe cover failed");
        }
    }
}

/// Best-effort wipe of an issue's per-page strip thumbnails (the
/// `<issue_id>/s/` subtree). Leaves the cover file at the thumbs root
/// untouched — used by "force recreate page thumbnails" flows that
/// shouldn't disturb the cover.
pub fn wipe_issue_strips(data_dir: &Path, issue_id: &str) {
    let dir = thumbs_root(data_dir).join(issue_id).join("s");
    if dir.exists()
        && let Err(e) = fs::remove_dir_all(&dir)
    {
        tracing::warn!(path = %dir.display(), error = %e, "wipe issue strips failed");
    }
}

/// Best-effort wipe of an issue's on-disk thumbnails (cover + strip dir),
/// across every known extension. Used by removal-confirmation, library
/// deletion, the orphan sweep, and the admin force-recreate / delete-all
/// flows. Errors are logged but never returned — thumbnails are
/// recoverable; the caller's primary action shouldn't fail because we
/// couldn't unlink a stale file.
pub fn wipe_issue_thumbs(data_dir: &Path, issue_id: &str) {
    wipe_issue_cover(data_dir, issue_id);
    let dir = issue_thumbs_dir(data_dir, issue_id);
    if dir.exists()
        && let Err(e) = fs::remove_dir_all(&dir)
    {
        tracing::warn!(path = %dir.display(), error = %e, "wipe issue dir failed");
    }
}

/// Enumerate issue ids that have on-disk thumbnails. Used by the orphan
/// sweep to cross-reference against the DB. Looks at both top-level cover
/// files (any known ext) and per-issue subdirs.
pub fn list_issues_on_disk(
    data_dir: &Path,
) -> Result<std::collections::HashSet<String>, std::io::Error> {
    let root = thumbs_root(data_dir);
    let mut out = std::collections::HashSet::new();
    if !root.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(&root)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let s = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_file() {
            // <id>.<ext> — strip extension and keep the stem if recognized.
            if let Some((stem, ext)) = s.rsplit_once('.')
                && KNOWN_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext))
            {
                out.insert(stem.to_owned());
            }
        } else if ft.is_dir() {
            out.insert(s.to_owned());
        }
    }
    Ok(out)
}

/// Generate the cover thumbnail. Idempotent — no-op if a file at the
/// target format already exists.
pub fn generate_cover(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    format: ThumbFormat,
) -> Result<PathBuf, ThumbError> {
    generate(data_dir, archive, issue_id, Variant::Cover, 0, format)
}

/// Generate a thumbnail for the page at `page_index` (0-based). Idempotent.
/// Legacy entry point used by the inline HTTP fallback — equivalent to
/// `generate(.., Cover, 0, format)` for n=0 and `generate(.., Strip, n,
/// format)` otherwise.
pub fn generate_page_thumb(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    page_index: usize,
    format: ThumbFormat,
) -> Result<PathBuf, ThumbError> {
    let variant = if page_index == 0 {
        Variant::Cover
    } else {
        Variant::Strip
    };
    generate(data_dir, archive, issue_id, variant, page_index, format)
}

/// Generate one (variant, page) thumbnail at the requested format.
/// Idempotent — returns `Ok(path)` without re-encoding if a file at the
/// target *format* already exists. A file in a *different* format does not
/// satisfy the request, so callers can switch formats by writing the new
/// extension alongside the old (or wiping first via the admin
/// force-recreate flow — preferred since old files would otherwise leak).
pub fn generate(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    variant: Variant,
    page_index: usize,
    format: ThumbFormat,
) -> Result<PathBuf, ThumbError> {
    generate_with_quality(
        data_dir,
        archive,
        issue_id,
        variant,
        page_index,
        format,
        ThumbnailQuality::default(),
    )
}

pub fn generate_with_quality(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    variant: Variant,
    page_index: usize,
    format: ThumbFormat,
    quality: ThumbnailQuality,
) -> Result<PathBuf, ThumbError> {
    let out = variant_path(data_dir, issue_id, variant, page_index, format);
    if out.exists() {
        return Ok(out);
    }
    let img = decode_page(archive, page_index)?;
    encode_variant_to_disk(&out, &img, variant, format, quality)
}

/// Read a page's bytes from the archive and decode into a `DynamicImage`.
/// Pulled out so `generate_all` can decode page 0 once and reuse it for
/// both cover and strip — the two variants only differ in resize/encode
/// parameters, not in the source pixels.
fn decode_page(
    archive: &mut dyn ComicArchive,
    page_index: usize,
) -> Result<DynamicImage, ThumbError> {
    let entry = archive
        .pages()
        .get(page_index)
        .copied()
        .cloned()
        .ok_or(ThumbError::PageOutOfRange)?;
    decode_entry(archive, &entry)
}

fn decode_entry(
    archive: &mut dyn ComicArchive,
    entry: &ArchiveEntry,
) -> Result<DynamicImage, ThumbError> {
    let bytes = archive.read_entry_bytes(&entry.name)?;
    decode_bytes(&bytes)
}

/// Decode an in-memory page payload. Lifted out of `decode_entry` so the
/// parallel strip pipeline can run decode on a worker thread without
/// holding the (non-Sync) archive handle.
fn decode_bytes(bytes: &[u8]) -> Result<DynamicImage, ThumbError> {
    let _span = tracing::trace_span!("thumb.decode", bytes = bytes.len()).entered();
    image::load_from_memory(bytes).map_err(|e| ThumbError::Decode(e.to_string()))
}

/// SIMD-accelerated downscale using `fast_image_resize`. Input is converted
/// to RGBA8 once (covers any source pixel layout `image` produces) and the
/// output is returned as raw RGBA8 bytes that the encoders consume directly
/// — skipping the `RgbaImage` wrapper / re-conversion the encoders would
/// otherwise force on us.
///
/// Returns `Ok(None)` when the source already fits under `max_w`; callers
/// fall back to the un-resized RGBA8 buffer in that case to avoid a
/// pointless copy.
fn resize_rgba(
    src: &image::RgbaImage,
    max_w: u32,
    filter: FirFilter,
) -> Result<Option<(Vec<u8>, u32, u32)>, ThumbError> {
    let (w, h) = (src.width(), src.height());
    if w <= max_w {
        return Ok(None);
    }
    let new_h = ((h as f32) * (max_w as f32 / w as f32)).round().max(1.0) as u32;
    let _span = tracing::trace_span!(
        "thumb.resize",
        from_w = w,
        from_h = h,
        to_w = max_w,
        to_h = new_h,
    )
    .entered();
    let src_ref = FirImageRef::new(w, h, src.as_raw(), PixelType::U8x4)
        .map_err(|e| ThumbError::Encode(format!("fir src: {e}")))?;
    let mut dst = FirImage::new(max_w, new_h, PixelType::U8x4);
    let mut resizer = Resizer::new();
    resizer
        .resize(
            &src_ref,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
        )
        .map_err(|e| ThumbError::Encode(format!("fir resize: {e}")))?;
    Ok(Some((dst.into_vec(), max_w, new_h)))
}

/// Resize a decoded image to the variant's max width and write it to
/// `out` in `format` via tmp-then-rename. Returns the final path.
fn encode_variant_to_disk(
    out: &Path,
    img: &DynamicImage,
    variant: Variant,
    format: ThumbFormat,
    quality: ThumbnailQuality,
) -> Result<PathBuf, ThumbError> {
    fs::create_dir_all(out.parent().expect("thumbs dir parent"))?;

    let (w, h) = img.dimensions();
    let max_w = variant.max_width();
    // Fast SIMD path: convert once to RGBA8, hand the bytes to fir, and
    // feed the same byte buffer straight to the encoder. The old `image`
    // resize was scalar Lanczos3 — typically the dominant cost for cover
    // generation on a 3000×2000 page.
    let src_rgba = img.to_rgba8();
    let (rgba_bytes, rw, rh) = match resize_rgba(&src_rgba, max_w, variant.filter())? {
        Some(resized) => resized,
        None => (src_rgba.into_raw(), w, h),
    };

    let quality = quality.for_variant(variant);
    let _span = tracing::trace_span!(
        "thumb.encode",
        format = format.as_str(),
        quality,
        w = rw,
        h = rh,
        filter = variant.filter_name(),
    )
    .entered();

    let buf: Vec<u8> = match format {
        ThumbFormat::Webp => {
            // libwebp via the `webp` crate. The `image` 0.25 codec was
            // lossless-only and produced files 5-10× larger at the same
            // visual quality; this path is what `THUMBNAIL_VERSION` v2
            // signaled callers to regenerate against.
            let encoder = webp::Encoder::from_rgba(&rgba_bytes, rw, rh);
            let mem = encoder.encode(quality as f32);
            mem.to_vec()
        }
        ThumbFormat::Jpeg => {
            // JPEG: drop the alpha channel (libjpeg can't carry it
            // without breaking decoders). The cover/strip use-case is
            // RGB-only anyway — a fully transparent comic page would
            // be a content bug.
            let rgb = rgba_to_rgb(&rgba_bytes);
            let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut cursor = Cursor::new(&mut buf);
            let enc = JpegEncoder::new_with_quality(&mut cursor, quality.max(1));
            enc.write_image(&rgb, rw, rh, image::ExtendedColorType::Rgb8)
                .map_err(|e| ThumbError::Encode(e.to_string()))?;
            buf
        }
        ThumbFormat::Png => {
            // PNG is lossless; quality() is meaningless here. Use the
            // `Fast` compression preset because PNG is the worst format
            // for this pipeline (5-10× larger than webp/jpeg) — there's
            // no payoff to spending CPU on max compression. Keep RGBA
            // so covers that use transparency around variant logos
            // round-trip correctly.
            let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut cursor = Cursor::new(&mut buf);
            let enc = PngEncoder::new_with_quality(
                &mut cursor,
                CompressionType::Fast,
                PngFilterType::NoFilter,
            );
            enc.write_image(&rgba_bytes, rw, rh, image::ExtendedColorType::Rgba8)
                .map_err(|e| ThumbError::Encode(e.to_string()))?;
            buf
        }
    };

    let tmp = out.with_extension(format!("{}.tmp", format.ext()));
    fs::write(&tmp, &buf)?;
    fs::rename(&tmp, out)?;
    Ok(out.to_path_buf())
}

/// In-place strip-alpha conversion: copies RGB triplets out of an RGBA
/// buffer into a new vec sized for the encoder. Runs before a JPEG encode
/// since libjpeg refuses to handle a 4th channel.
fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    let pixels = rgba.len() / 4;
    let mut out = Vec::with_capacity(pixels * 3);
    for chunk in rgba.chunks_exact(4) {
        out.extend_from_slice(&chunk[..3]);
    }
    out
}

/// Generate every artifact for an issue: the cover plus a strip thumb for
/// each page, all in `format`. Idempotent and resilient to partial state —
/// already-generated pages (in the same format) are skipped. Returns the
/// per-page error count so the caller can surface persistent failures.
///
/// Page 0 is decoded once and used for both the cover and the strip-page-0
/// thumbnails — they share source pixels and only differ in resize/encode
/// parameters. Pages 1..N go through `generate()` which reads + decodes
/// each separately.
pub fn generate_all(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    format: ThumbFormat,
) -> Result<GenerateAllOutcome, ThumbError> {
    generate_all_with_quality(
        data_dir,
        archive,
        issue_id,
        format,
        ThumbnailQuality::default(),
    )
}

pub fn generate_all_with_quality(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    format: ThumbFormat,
    quality: ThumbnailQuality,
) -> Result<GenerateAllOutcome, ThumbError> {
    let _span = tracing::info_span!(
        "thumb.generate_all",
        issue_id = %issue_id,
        format = format.as_str(),
    )
    .entered();

    let pages: Vec<ArchiveEntry> = archive.pages().into_iter().cloned().collect();
    let total_pages = pages.len();

    // Decode page 0 once. Both cover and strip-page-0 reuse it, skipping a
    // second zip read + decode. Skip the work entirely if both target files
    // are already on disk.
    let cover_out = variant_path(data_dir, issue_id, Variant::Cover, 0, format);
    let strip0_out = variant_path(data_dir, issue_id, Variant::Strip, 0, format);
    if !(cover_out.exists() && strip0_out.exists()) {
        let page0 = pages.first().ok_or(ThumbError::PageOutOfRange)?;
        let page0 = decode_entry(archive, page0)?;
        if !cover_out.exists() {
            encode_variant_to_disk(&cover_out, &page0, Variant::Cover, format, quality)?;
        }
        if !strip0_out.exists() {
            encode_variant_to_disk(&strip0_out, &page0, Variant::Strip, format, quality)?;
        }
    }

    // Pages 1..N: serial archive reads (handle isn't Sync), then fan out
    // decode + resize + encode + write across rayon's worker pool.
    let pending = collect_pending_strip_bytes(
        archive,
        &pages,
        |n| variant_path(data_dir, issue_id, Variant::Strip, n, format),
        1,
    );
    let failed = parallel_encode_strips(data_dir, issue_id, format, quality, pending);

    Ok(GenerateAllOutcome {
        total_pages,
        failed,
    })
}

/// Generate only the reader page-strip thumbnails. Used by the lazy reader
/// catchup job so scan/admin backfills do not eagerly encode every page in a
/// large library.
pub fn generate_strips(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    format: ThumbFormat,
) -> Result<GenerateAllOutcome, ThumbError> {
    generate_strips_with_quality(
        data_dir,
        archive,
        issue_id,
        format,
        ThumbnailQuality::default(),
    )
}

pub fn generate_strips_with_quality(
    data_dir: &Path,
    archive: &mut dyn ComicArchive,
    issue_id: &str,
    format: ThumbFormat,
    quality: ThumbnailQuality,
) -> Result<GenerateAllOutcome, ThumbError> {
    let _span = tracing::info_span!(
        "thumb.generate_strips",
        issue_id = %issue_id,
        format = format.as_str(),
    )
    .entered();

    let pages: Vec<ArchiveEntry> = archive.pages().into_iter().cloned().collect();
    if pages.is_empty() {
        return Err(ThumbError::PageOutOfRange);
    }

    let total_pages = pages.len();
    let pending = collect_pending_strip_bytes(
        archive,
        &pages,
        |n| variant_path(data_dir, issue_id, Variant::Strip, n, format),
        0,
    );
    let failed = parallel_encode_strips(data_dir, issue_id, format, quality, pending);

    Ok(GenerateAllOutcome {
        total_pages,
        failed,
    })
}

/// Phase 1 of the parallel strip pipeline: serially read every page that
/// needs encoding into memory, skipping pages whose target file already
/// exists. Returns the (page_index, bytes) pairs the parallel encoder
/// should process. Read errors are swallowed with a warn-level trace and
/// the page is dropped — the catchup sweep can pick it up next time.
///
/// `start` lets `generate_all` skip page 0 (which it already encoded
/// inline via the cover/strip-0 shared decode path).
///
/// Memory cost is bounded by the archive's total page bytes for one
/// issue (typically 30-80 MB). The post-scan worker semaphore already
/// gates concurrent issues, so total memory across the worker pool stays
/// reasonable without an explicit bounded channel.
fn collect_pending_strip_bytes<F>(
    archive: &mut dyn ComicArchive,
    pages: &[ArchiveEntry],
    out_path_for: F,
    start: usize,
) -> Vec<(usize, Vec<u8>)>
where
    F: Fn(usize) -> PathBuf,
{
    let _span = tracing::trace_span!("thumb.read_pages", count = pages.len() - start).entered();
    let mut pending = Vec::with_capacity(pages.len().saturating_sub(start));
    for (n, entry) in pages.iter().enumerate().skip(start) {
        if out_path_for(n).exists() {
            continue;
        }
        match archive.read_entry_bytes(&entry.name) {
            Ok(bytes) => pending.push((n, bytes)),
            Err(e) => {
                tracing::warn!(page = n, error = %e, "strip read failed");
            }
        }
    }
    pending
}

/// Phase 2: rayon-parallel decode + resize + encode + atomic write. Each
/// worker is independent — decode and write to its own file via
/// tmp+rename, so there's no shared mutable state across the pool.
fn parallel_encode_strips(
    data_dir: &Path,
    issue_id: &str,
    format: ThumbFormat,
    quality: ThumbnailQuality,
    pending: Vec<(usize, Vec<u8>)>,
) -> Vec<(usize, String)> {
    if pending.is_empty() {
        return Vec::new();
    }
    let _span = tracing::trace_span!("thumb.parallel_encode", count = pending.len()).entered();
    pending
        .into_par_iter()
        .filter_map(|(n, bytes)| {
            let out = variant_path(data_dir, issue_id, Variant::Strip, n, format);
            let result = decode_bytes(&bytes).and_then(|img| {
                encode_variant_to_disk(&out, &img, Variant::Strip, format, quality)
            });
            match result {
                Ok(_) => None,
                Err(e) => Some((n, e.to_string())),
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct GenerateAllOutcome {
    pub total_pages: usize,
    /// `(page_index, error_message)` for any per-page failures. The cover
    /// is generated up-front and propagates as `Err(_)` from `generate_all`
    /// since a missing cover is a real failure; per-page strip misses are
    /// tolerated so a single corrupt page doesn't lose every other thumb.
    pub failed: Vec<(usize, String)>,
}
