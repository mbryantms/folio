//! Bounded image decoding (SEC-2).
//!
//! `image::load_from_memory` decodes with `Limits::default()` — no width/height
//! cap and a 512 MiB allocation ceiling. The archive layer's byte caps
//! (`max_entry_bytes` / `max_compression_ratio` / `max_total_bytes`) operate on
//! the *compressed* zip stream, so a tiny crafted page (e.g. a 30 KB PNG whose
//! header declares 30000×30000) passes every archive cap at ~1:1 and then forces
//! a multi-hundred-MB pixel buffer. Because that decode runs on *every*
//! thumbnail render, OCR call, pHash, and page transform of the owning issue —
//! several of which decode concurrently in the post-scan / deep-validate
//! pipelines — one crafted page becomes a repeatable memory-amplification DoS,
//! OOM-killing a small host.
//!
//! [`decode_limited`] is the single entry point all decode sites use. It caps
//! both dimensions and the per-decode allocation, rejecting an oversized header
//! before any pixel buffer is reserved.

use image::DynamicImage;

/// Maximum pixels accepted on either axis. 20k px on the long edge comfortably
/// clears any legitimate scan — a 600-DPI tabloid (11×17") page is ~10200 px —
/// while rejecting the dimension-bomb headers a decode bomb relies on.
pub const MAX_DECODE_DIMENSION: u32 = 20_000;

/// Hard ceiling on the intermediate allocation a single decode may request,
/// well under the image-crate default of 512 MiB.
pub const MAX_DECODE_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

/// Decode an in-memory image with explicit dimension + allocation limits.
///
/// Replaces `image::load_from_memory` at every decode site so the limits below
/// are always applied. Returns the same [`image::ImageError`] surface, so an
/// oversized or otherwise-undecodable image fails cleanly as a decode error
/// rather than allocating.
pub fn decode_limited(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    decode_with_limits(bytes, MAX_DECODE_DIMENSION, MAX_DECODE_ALLOC_BYTES)
}

/// Inner decode with caller-supplied caps. Split out so tests can exercise the
/// limit-enforcement path against a low cap without crafting malicious bytes.
fn decode_with_limits(
    bytes: &[u8],
    max_dimension: u32,
    max_alloc: u64,
) -> image::ImageResult<DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_dimension);
    limits.max_image_height = Some(max_dimension);
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);

    reader.decode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbaImage};

    fn png_16x16() -> Vec<u8> {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(16, 16));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn decodes_within_limits() {
        let png = png_16x16();
        let img = decode_limited(&png).expect("16x16 png decodes");
        assert_eq!((img.width(), img.height()), (16, 16));
    }

    #[test]
    fn rejects_image_exceeding_dimension_cap() {
        // A real, valid 16x16 PNG is rejected when the cap is set below its
        // dimensions — proving the limit is wired into the decoder and enforced
        // before allocation, which is exactly what stops a dimension bomb.
        let png = png_16x16();
        let err = decode_with_limits(&png, 8, MAX_DECODE_ALLOC_BYTES);
        assert!(
            matches!(err, Err(image::ImageError::Limits(_))),
            "expected a Limits error, got {err:?}",
        );
    }

    #[test]
    fn rejects_image_exceeding_alloc_cap() {
        // Same image, but starved of allocation budget — the other axis of the
        // guard.
        let png = png_16x16();
        let err = decode_with_limits(&png, MAX_DECODE_DIMENSION, 16);
        assert!(
            matches!(err, Err(image::ImageError::Limits(_))),
            "expected a Limits error, got {err:?}",
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_limited(b"not an image").is_err());
    }
}
