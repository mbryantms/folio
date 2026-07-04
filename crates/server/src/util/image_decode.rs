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

/// Decode an in-memory image with explicit dimension + allocation limits,
/// **with EXIF orientation applied**.
///
/// Replaces `image::load_from_memory` at every decode site so the limits below
/// are always applied. Returns the same [`image::ImageError`] surface, so an
/// oversized or otherwise-undecodable image fails cleanly as a decode error
/// rather than allocating.
///
/// Orientation matters because browsers render `<img>` bytes with
/// `image-orientation: from-image` (the default) — an EXIF-tagged page in
/// the reader's full-res view displays rotated, while `image`'s
/// `ImageReader::decode()` returns the raw sensor pixels. Every
/// server-produced derivative (thumbnails, `?w=` variants, page-edit
/// re-encodes, OCR crops) therefore disagreed with the reader by exactly
/// the EXIF rotation for such pages. Applying the orientation here makes
/// every decode site agree with what the browser shows; re-encodes emit no
/// EXIF, so their output is upright by construction.
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
    use image::ImageDecoder;
    use image::metadata::Orientation;

    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_dimension);
    limits.max_image_height = Some(max_dimension);
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);

    let mut decoder = reader.into_decoder()?;
    // Orientation must be read before the pixels; a missing/garbled EXIF
    // block is a no-op, not an error.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Extract the EXIF orientation value (1..=8) from a JPEG byte prefix, or
/// `None` when absent/unparseable. Hand-rolled because the scanner's
/// dimension probe reads only the first 256 KB of each page and parses
/// header dimensions manually — full decode is off the table there, but an
/// orientation of 5..=8 means the *displayed* image has swapped axes, and
/// the probe's width/height feed `double_page` inference and the reader's
/// layout reservation.
pub fn jpeg_exif_orientation(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }
    let mut i = 2usize;
    // Walk JPEG segments looking for APP1/Exif. Stop at SOS (entropy data).
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xff {
            return None;
        }
        let marker = bytes[i + 1];
        if marker == 0xda {
            return None; // start of scan — no EXIF before pixels
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            return None;
        }
        if marker == 0xe1 {
            let seg = &bytes[i + 4..i + 2 + len];
            if let Some(tiff) = seg.strip_prefix(b"Exif\0\0") {
                return tiff_orientation(tiff);
            }
        }
        i += 2 + len;
    }
    None
}

/// Read tag 0x0112 (Orientation) from IFD0 of a TIFF blob (EXIF body).
fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
    if tiff.len() < 14 {
        return None;
    }
    let le = match &tiff[0..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(o..o + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(o..o + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd0 = u32_at(4)? as usize;
    let count = u16_at(ifd0)? as usize;
    for n in 0..count {
        let entry = ifd0 + 2 + n * 12;
        if u16_at(entry)? == 0x0112 {
            // SHORT, count 1 → value lives inline in the offset field.
            let v = u16_at(entry + 8)?;
            return (1..=8).contains(&v).then_some(v);
        }
    }
    None
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

    /// Splice a minimal EXIF APP1 segment (Orientation = `orient`) into a
    /// freshly-encoded baseline JPEG, right after SOI. 26-byte TIFF body:
    /// II header + one IFD0 entry (tag 0x0112, SHORT, inline value).
    fn jpeg_with_orientation(w: u32, h: u32, orient: u16) -> Vec<u8> {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(w, h));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        let jpeg = buf.into_inner();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);

        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(b"II\x2a\x00"); // little-endian TIFF
        tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_le_bytes()); // count
        tiff.extend_from_slice(&orient.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]); // value padding
        tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD

        let payload_len = 6 + tiff.len(); // "Exif\0\0" + TIFF
        let mut out = Vec::with_capacity(jpeg.len() + payload_len + 4);
        out.extend_from_slice(&jpeg[..2]); // SOI
        out.extend_from_slice(&[0xff, 0xe1]); // APP1
        out.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
        out.extend_from_slice(b"Exif\x00\x00");
        out.extend_from_slice(&tiff);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn decode_applies_exif_orientation() {
        // Orientation 6 = Rotate90 CW on display: a raw 16x8 landscape
        // JPEG must decode as 8x16 portrait, matching how a browser
        // renders the same bytes. This is what keeps thumbnails /
        // variants / page-edit re-encodes in agreement with the
        // reader's full-res <img>.
        let jpeg = jpeg_with_orientation(16, 8, 6);
        let img = decode_limited(&jpeg).expect("oriented jpeg decodes");
        assert_eq!(
            (img.width(), img.height()),
            (8, 16),
            "orientation 6 must swap the displayed axes"
        );
        // Orientation 1 (upright) and missing EXIF are no-ops.
        let upright = jpeg_with_orientation(16, 8, 1);
        let img = decode_limited(&upright).unwrap();
        assert_eq!((img.width(), img.height()), (16, 8));
    }

    #[test]
    fn jpeg_exif_orientation_parses_and_rejects() {
        assert_eq!(
            jpeg_exif_orientation(&jpeg_with_orientation(16, 8, 6)),
            Some(6)
        );
        assert_eq!(
            jpeg_exif_orientation(&jpeg_with_orientation(16, 8, 8)),
            Some(8)
        );
        // Plain JPEG without EXIF → None.
        let img = DynamicImage::ImageRgba8(RgbaImage::new(4, 4));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Jpeg).unwrap();
        assert_eq!(jpeg_exif_orientation(&buf.into_inner()), None);
        // Non-JPEG bytes → None.
        assert_eq!(jpeg_exif_orientation(b"not a jpeg"), None);
    }

    #[test]
    fn decode_limited_rejects_dimension_bomb_at_production_cap() {
        // A (MAX_DECODE_DIMENSION + 1) x 1 image is trivially cheap to allocate
        // and encode (~20k pixels) yet its width exceeds the production cap. The
        // guard `decode_limited` — the exact function ARC-1 routes the
        // deep-validate and provider-cover decodes through — must reject it via
        // the header dimension check *before* any pixel buffer is reserved.
        let img = DynamicImage::ImageRgba8(RgbaImage::new(MAX_DECODE_DIMENSION + 1, 1));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        let err = decode_limited(buf.get_ref());
        assert!(
            matches!(err, Err(image::ImageError::Limits(_))),
            "expected a Limits error at the production cap, got {err:?}",
        );
    }
}
