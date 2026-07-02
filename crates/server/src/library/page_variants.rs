//! On-the-fly reader page-size variants + on-disk cache (audit FEP-1,
//! decision D1). Design note: `docs/dev/page-variants.md`.
//!
//! A variant is a WebP re-encode of one archive page downscaled to a fixed
//! width tier. Variants are rendered lazily on first request and cached at
//! `data_path/cache/pages/{content_hash}/{page}-w{tier}.webp`. The cache is
//! LRU-bounded by a byte budget (`COMIC_PAGE_VARIANT_CACHE_BYTES`); keys
//! embed `content_hash`, so an archive edit orphans its old variants and
//! the sweep ages them out.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use fast_image_resize::{
    FilterType as FirFilter, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image as FirImage, ImageRef as FirImageRef},
};

use crate::util::image_decode::decode_limited;

/// The fixed width ladder. Kept small so every device shape converges on
/// the same few cache entries; must stay sorted ascending. Mirrored by
/// `PAGE_VARIANT_TIERS` in `web/lib/urls.ts` — change both together.
pub const TIERS: &[u32] = &[480, 720, 1080, 1600];

/// WebP quality for variants — same as the thumbnail pipeline's covers.
const WEBP_QUALITY: f32 = 80.0;

/// Clamp a requested width to the ladder: the smallest tier ≥ the ask,
/// or the largest tier when the ask exceeds the ladder.
pub fn clamp_to_tier(w: u32) -> u32 {
    for t in TIERS {
        if *t >= w {
            return *t;
        }
    }
    *TIERS.last().expect("TIERS non-empty")
}

/// Cache file for a `(content_hash, page, tier)` triple.
pub fn cache_path(data_path: &Path, content_hash: &str, page: usize, tier: u32) -> PathBuf {
    data_path
        .join("cache")
        .join("pages")
        .join(content_hash)
        .join(format!("{page}-w{tier}.webp"))
}

/// Outcome of rendering a variant from original page bytes.
pub enum Rendered {
    /// Downscaled + re-encoded WebP bytes.
    Webp(Vec<u8>),
    /// The source is already ≤ the tier — serve the original bytes
    /// verbatim (never upscale, never re-encode).
    OriginalIsSmaller,
}

/// Decode (capped), downscale to `tier` width, encode WebP. CPU-bound —
/// callers run this inside `spawn_blocking`. The `decode_limited` caps
/// (20k px / 256 MiB) apply BEFORE any resize, so a dimension bomb dies
/// at decode exactly like the full-res paths.
pub fn render_variant(original: &[u8], tier: u32) -> Result<Rendered, String> {
    let img = decode_limited(original).map_err(|e| format!("decode: {e}"))?;
    let rgba = img.into_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w <= tier {
        return Ok(Rendered::OriginalIsSmaller);
    }
    let new_h = ((h as f32) * (tier as f32 / w as f32)).round().max(1.0) as u32;
    let src = FirImageRef::new(w, h, rgba.as_raw(), PixelType::U8x4)
        .map_err(|e| format!("fir src: {e}"))?;
    let mut dst = FirImage::new(tier, new_h, PixelType::U8x4);
    let mut resizer = Resizer::new();
    resizer
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FirFilter::Lanczos3)),
        )
        .map_err(|e| format!("fir resize: {e}"))?;
    let encoder = webp::Encoder::from_rgba(dst.buffer(), tier, new_h);
    Ok(Rendered::Webp(encoder.encode(WEBP_QUALITY).to_vec()))
}

/// Atomic cache write: tmp in the same dir → fsync → rename. A failed
/// write is soft (the variant streams from memory regardless); the next
/// miss retries.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().expect("cache path has parent");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        path.file_name().and_then(|n| n.to_str()).unwrap_or("v")
    ));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Bump a cache file's mtime so the LRU sweep sees it as recently used.
/// atime is unreliable (noatime mounts), mtime we control.
pub fn touch(path: &Path) {
    let now = std::fs::FileTimes::new().set_modified(SystemTime::now());
    if let Ok(f) = std::fs::File::options().append(true).open(path) {
        let _ = f.set_times(now);
    }
}

/// One sweep at a time per process; misses during a sweep just skip —
/// the next write re-triggers.
static SWEEP_RUNNING: AtomicBool = AtomicBool::new(false);

/// Fire-and-forget budget enforcement after a cache write. Walks
/// `cache/pages`, and when the total exceeds `budget_bytes`, deletes
/// oldest-mtime files until the total is ≤ 90% of budget (hysteresis so
/// every write doesn't trigger a walk at the boundary). Empty hash dirs
/// are pruned opportunistically — that's also how edited archives' old
/// variants leave the disk.
pub fn spawn_evict_if_needed(data_path: PathBuf, budget_bytes: u64) {
    if budget_bytes == 0 {
        return; // caching disabled; nothing accumulates
    }
    if SWEEP_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::task::spawn_blocking(move || {
        let result = evict_to_budget(&data_path.join("cache").join("pages"), budget_bytes);
        SWEEP_RUNNING.store(false, Ordering::Release);
        if let Err(e) = result {
            tracing::warn!(error = %e, "page-variant cache sweep failed");
        }
    });
}

fn evict_to_budget(root: &Path, budget_bytes: u64) -> std::io::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    // (mtime, size, path) for every cache file.
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let mut total: u64 = 0;
    for hash_dir in std::fs::read_dir(root)? {
        let hash_dir = hash_dir?;
        if !hash_dir.file_type()?.is_dir() {
            continue;
        }
        let mut empty = true;
        for entry in std::fs::read_dir(hash_dir.path())? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if !meta.is_file() {
                continue;
            }
            empty = false;
            total += meta.len();
            files.push((
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                meta.len(),
                entry.path(),
            ));
        }
        if empty {
            let _ = std::fs::remove_dir(hash_dir.path());
        }
    }
    if total <= budget_bytes {
        return Ok(());
    }
    let target = budget_bytes.saturating_mul(9) / 10;
    files.sort_by_key(|(mtime, _, _)| *mtime);
    let mut evicted = 0u64;
    let mut count = 0u32;
    for (_, size, path) in files {
        if total - evicted <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            evicted += size;
            count += 1;
            if let Some(dir) = path.parent() {
                let _ = std::fs::remove_dir(dir); // fails (kept) unless empty
            }
        }
    }
    tracing::debug!(
        evicted_files = count,
        evicted_bytes = evicted,
        total_before = total,
        budget = budget_bytes,
        "page-variant cache sweep"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_to_ladder() {
        assert_eq!(clamp_to_tier(1), 480);
        assert_eq!(clamp_to_tier(480), 480);
        assert_eq!(clamp_to_tier(481), 720);
        assert_eq!(clamp_to_tier(1080), 1080);
        assert_eq!(clamp_to_tier(99_999), 1600);
    }

    #[test]
    fn renders_webp_downscale_and_skips_upscale() {
        // 800×600 PNG → tier 480 downsizes; tier 1080 refuses to upscale.
        let mut img = image::RgbaImage::new(800, 600);
        for p in img.pixels_mut() {
            *p = image::Rgba([120, 10, 200, 255]);
        }
        let mut png: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        match render_variant(&png, 480).unwrap() {
            Rendered::Webp(bytes) => {
                let out = image::load_from_memory(&bytes).unwrap();
                assert_eq!(out.width(), 480);
                assert_eq!(out.height(), 360);
            }
            Rendered::OriginalIsSmaller => panic!("should downscale"),
        }
        assert!(matches!(
            render_variant(&png, 1080).unwrap(),
            Rendered::OriginalIsSmaller
        ));
    }

    #[test]
    fn eviction_respects_budget_and_lru_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cache").join("pages");
        let old = root.join("hash-a");
        let new = root.join("hash-b");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let old_f = old.join("0-w480.webp");
        let new_f = new.join("0-w480.webp");
        std::fs::write(&old_f, vec![0u8; 600]).unwrap();
        std::fs::write(&new_f, vec![0u8; 600]).unwrap();
        // Make `old_f` clearly older.
        let past = SystemTime::now() - std::time::Duration::from_secs(3600);
        let f = std::fs::File::options().append(true).open(&old_f).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(past))
            .unwrap();
        drop(f);

        // Budget 1000: total 1200 → evict down to ≤900 → the old file goes,
        // the new one stays, and the emptied hash dir is pruned.
        evict_to_budget(&root, 1000).unwrap();
        assert!(!old_f.exists(), "oldest-mtime file evicted");
        assert!(new_f.exists(), "recent file kept");
        assert!(!old.exists(), "emptied hash dir pruned");
    }
}
