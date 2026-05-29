//! Per-page image transforms for the archive page editor
//! (`archive-rewrite-1.0` M5).
//!
//! A [`TransformStep`] chain is attached to a page via
//! [`crate::jobs::archive_edit::PageOp::Transform`] and applied at rewrite
//! time, between the rotate and the re-encode steps in
//! [`crate::jobs::archive_edit::transform_image`]. The web editor mirrors
//! each step in `web/lib/image-transforms.ts` for a live canvas preview, so
//! the operator sees on the thumb roughly what the server will write.
//!
//! All bounds clamping happens here in [`apply_chain`] — pixel dimensions
//! are only known at apply time, so the op-validation pass
//! (`simulate_ops`) can't range-check a crop box. Every step is therefore
//! defined to be total: out-of-range inputs clamp or no-op rather than
//! erroring, so a malformed chain can never abort a rewrite.

use image::{DynamicImage, GenericImageView};
use serde::{Deserialize, Serialize};

/// One image adjustment applied to a single page. Tagged the same way as
/// [`crate::jobs::archive_edit::PageOp`] so the web client can emit a
/// discriminated union.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformStep {
    /// Additive brightness + multiplicative contrast, each on a -100..=100
    /// scale (0 = no change). Hand-rolled so the JS preview can match it
    /// exactly.
    BrightnessContrast { brightness: i32, contrast: i32 },
    /// Per-channel levels stretch: map the input range `[lo, hi]` onto the
    /// full `[0, 255]`. No-op unless `lo < hi`.
    LevelsClip { lo: u8, hi: u8 },
    /// Unsharp-mask sharpen. `amount` is the gaussian sigma, clamped to
    /// `0.0..=5.0`; `<= 0` is a no-op.
    Sharpen { amount: f32 },
    /// Median-filter denoise with a square window of the given pixel
    /// radius, clamped to `1..=4`.
    Despeckle { radius: u32 },
    /// Crop to the box `(x, y, w, h)` in source-pixel coordinates. Clamped
    /// to the image bounds; a zero-area result after clamping is a no-op.
    CropBox { x: u32, y: u32, w: u32, h: u32 },
}

/// Brightness/contrast input scale bound.
const BC_LIMIT: i32 = 100;
/// Max unsharp sigma.
const SHARPEN_MAX: f32 = 5.0;
/// Max median-filter radius (window grows as `(2r+1)^2`, so keep it small).
const DESPECKLE_MAX: u32 = 4;

impl TransformStep {
    fn apply(&self, img: DynamicImage) -> DynamicImage {
        match *self {
            TransformStep::BrightnessContrast {
                brightness,
                contrast,
            } => brightness_contrast(img, brightness, contrast),
            TransformStep::LevelsClip { lo, hi } => levels_clip(img, lo, hi),
            TransformStep::Sharpen { amount } => sharpen(img, amount),
            TransformStep::Despeckle { radius } => despeckle(img, radius),
            TransformStep::CropBox { x, y, w, h } => crop_box(img, x, y, w, h),
        }
    }
}

/// Fold a transform chain over `img` in order. Total: each step clamps its
/// own inputs, so any chain produces a valid (non-empty) image.
pub fn apply_chain(img: DynamicImage, chain: &[TransformStep]) -> DynamicImage {
    chain.iter().fold(img, |acc, step| step.apply(acc))
}

/// Per-channel brightness (additive) + contrast (multiplicative about the
/// 128 mid-grey). `brightness` / `contrast` are clamped to `-100..=100`.
/// The contrast factor mirrors the classic GIMP-style curve so the JS
/// preview can reproduce it byte-for-byte.
fn brightness_contrast(img: DynamicImage, brightness: i32, contrast: i32) -> DynamicImage {
    let b = brightness.clamp(-BC_LIMIT, BC_LIMIT);
    let c = contrast.clamp(-BC_LIMIT, BC_LIMIT) as f32;
    if b == 0 && c == 0.0 {
        return img;
    }
    // Map contrast -100..100 → factor. +100 → 259/... steep; -100 → flat.
    let factor = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
    let lut: [u8; 256] = std::array::from_fn(|i| {
        let v = factor * (i as f32 - 128.0) + 128.0 + b as f32;
        v.round().clamp(0.0, 255.0) as u8
    });
    let mut rgb = img.to_rgb8();
    for p in rgb.pixels_mut() {
        p.0[0] = lut[p.0[0] as usize];
        p.0[1] = lut[p.0[1] as usize];
        p.0[2] = lut[p.0[2] as usize];
    }
    DynamicImage::ImageRgb8(rgb)
}

/// Stretch `[lo, hi]` to `[0, 255]` per channel. No-op when `lo >= hi`.
fn levels_clip(img: DynamicImage, lo: u8, hi: u8) -> DynamicImage {
    if lo >= hi {
        return img;
    }
    let span = (hi - lo) as f32;
    let lut: [u8; 256] = std::array::from_fn(|i| {
        let v = (i as f32 - lo as f32) / span * 255.0;
        v.round().clamp(0.0, 255.0) as u8
    });
    let mut rgb = img.to_rgb8();
    for p in rgb.pixels_mut() {
        p.0[0] = lut[p.0[0] as usize];
        p.0[1] = lut[p.0[1] as usize];
        p.0[2] = lut[p.0[2] as usize];
    }
    DynamicImage::ImageRgb8(rgb)
}

/// Unsharp mask via the `image` crate's built-in `unsharpen` (gaussian
/// blur + weighted re-add). `amount` is the sigma, clamped to `0..=5`;
/// `<= 0` is a no-op.
fn sharpen(img: DynamicImage, amount: f32) -> DynamicImage {
    let sigma = amount.clamp(0.0, SHARPEN_MAX);
    if sigma <= 0.0 {
        return img;
    }
    let rgb = img.to_rgb8();
    DynamicImage::ImageRgb8(image::imageops::unsharpen(&rgb, sigma, 0))
}

/// Median filter with a square window of `radius` (clamped `1..=4`).
fn despeckle(img: DynamicImage, radius: u32) -> DynamicImage {
    let r = radius.clamp(1, DESPECKLE_MAX);
    let rgb = img.to_rgb8();
    DynamicImage::ImageRgb8(imageproc::filter::median_filter(&rgb, r, r))
}

/// Crop to `(x, y, w, h)`, clamped to the image. A zero-area result after
/// clamping (box fully outside, or zero w/h) leaves the image unchanged so
/// the rewrite never emits an empty page.
fn crop_box(img: DynamicImage, x: u32, y: u32, w: u32, h: u32) -> DynamicImage {
    let (iw, ih) = img.dimensions();
    if x >= iw || y >= ih {
        return img;
    }
    let cw = w.min(iw - x);
    let ch = h.min(ih - y);
    if cw == 0 || ch == 0 {
        return img;
    }
    if cw == iw && ch == ih {
        return img;
    }
    img.crop_imm(x, y, cw, ch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// A 16×16 horizontal gradient (column index → grey level scaled).
    fn gradient() -> DynamicImage {
        let mut img = RgbImage::new(16, 16);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let v = (x * 16) as u8;
            *p = Rgb([v, v, v]);
        }
        DynamicImage::ImageRgb8(img)
    }

    fn mean_luma(img: &DynamicImage) -> f32 {
        let rgb = img.to_rgb8();
        let sum: u64 = rgb.pixels().map(|p| p.0[0] as u64).sum();
        sum as f32 / (rgb.width() * rgb.height()) as f32
    }

    #[test]
    fn brightness_raises_and_lowers_mean() {
        let base = mean_luma(&gradient());
        let up = mean_luma(&brightness_contrast(gradient(), 50, 0));
        let down = mean_luma(&brightness_contrast(gradient(), -50, 0));
        assert!(
            up > base,
            "brightness +50 should raise mean ({up} vs {base})"
        );
        assert!(
            down < base,
            "brightness -50 should lower mean ({down} vs {base})"
        );
    }

    #[test]
    fn brightness_contrast_zero_is_noop() {
        let out = brightness_contrast(gradient(), 0, 0);
        assert_eq!(out.to_rgb8(), gradient().to_rgb8());
    }

    #[test]
    fn levels_clip_maps_endpoints() {
        // Stretch [64, 192] → [0, 255].
        let out = levels_clip(gradient(), 64, 192).to_rgb8();
        // A pixel that was <= 64 clamps to 0; one >= 192 clamps to 255.
        assert_eq!(out.get_pixel(4, 0).0[0], 0); // x=4 → 64 → 0
        assert_eq!(out.get_pixel(12, 0).0[0], 255); // x=12 → 192 → 255
    }

    #[test]
    fn levels_clip_noop_when_lo_ge_hi() {
        let out = levels_clip(gradient(), 200, 100);
        assert_eq!(out.to_rgb8(), gradient().to_rgb8());
    }

    #[test]
    fn sharpen_keeps_dims_and_is_noop_at_zero() {
        let sharp = sharpen(gradient(), 2.0);
        assert_eq!(sharp.dimensions(), (16, 16));
        let noop = sharpen(gradient(), 0.0);
        assert_eq!(noop.to_rgb8(), gradient().to_rgb8());
    }

    #[test]
    fn despeckle_removes_single_pixel_noise() {
        let mut img = RgbImage::from_pixel(8, 8, Rgb([10, 10, 10]));
        img.put_pixel(4, 4, Rgb([250, 250, 250])); // lone speck
        let out = despeckle(DynamicImage::ImageRgb8(img), 1).to_rgb8();
        assert_eq!(
            out.get_pixel(4, 4).0[0],
            10,
            "median filter should erase a lone bright speck"
        );
    }

    #[test]
    fn crop_box_yields_exact_dims_and_clamps() {
        let out = crop_box(gradient(), 2, 2, 8, 8);
        assert_eq!(out.dimensions(), (8, 8));
        // Oversized box clamps to the image bounds.
        let clamped = crop_box(gradient(), 8, 8, 999, 999);
        assert_eq!(clamped.dimensions(), (8, 8));
        // Fully out-of-bounds → unchanged.
        let oob = crop_box(gradient(), 99, 99, 4, 4);
        assert_eq!(oob.dimensions(), (16, 16));
    }

    #[test]
    fn chain_is_deterministic() {
        let chain = vec![
            TransformStep::BrightnessContrast {
                brightness: 20,
                contrast: 30,
            },
            TransformStep::LevelsClip { lo: 10, hi: 240 },
            TransformStep::CropBox {
                x: 1,
                y: 1,
                w: 10,
                h: 10,
            },
        ];
        let a = apply_chain(gradient(), &chain).to_rgb8();
        let b = apply_chain(gradient(), &chain).to_rgb8();
        assert_eq!(a, b);
        assert_eq!(a.dimensions(), (10, 10));
    }
}
