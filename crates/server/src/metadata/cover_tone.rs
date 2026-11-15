//! A cover's two colours: its average, and its most vibrant.
//!
//! The web app shows a cover's average colour as its placeholder while
//! the image loads, and washes a hovered card in the vibrant one
//! (`web/lib/ui/cover-tone.ts`). The browser can read those from an image
//! it has decoded, but only once it has, and never from a cross-origin
//! image whose host sends no CORS headers. Computing them here puts the
//! colours in the first response instead.
//!
//! The pick mirrors `paletteFromPixels` in `web/lib/ui/cover-palette.ts`
//! step for step (the same 24 x 36 sample, the same score), so a colour
//! computed on the server and one the browser computed for the same
//! cover agree to within the resampling filter.

use image::{DynamicImage, imageops::FilterType};

/// The sample the colours are read from. A cover is 2:3.
const SAMPLE_W: u32 = 24;
const SAMPLE_H: u32 = 36;

/// A pixel counts only when it is at least half opaque.
const MIN_ALPHA: u8 = 128;

/// Below this score nothing on the cover is colourful enough to stand
/// for it, and the vibrant colour falls back to the average.
const MIN_VIBRANT_SCORE: f32 = 0.15;

/// A cover's average and most vibrant colours, as sRGB bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverTone {
    pub average: [u8; 3],
    pub vibrant: [u8; 3],
}

impl CoverTone {
    /// Twelve lowercase hex digits: the average, then the vibrant colour.
    /// The web app's `decodeTone` reads exactly this.
    pub fn encode(&self) -> String {
        let [ar, ag, ab] = self.average;
        let [vr, vg, vb] = self.vibrant;
        format!("{ar:02x}{ag:02x}{ab:02x}{vr:02x}{vg:02x}{vb:02x}")
    }
}

/// True for a string [`CoverTone::encode`] could have written. Stored
/// values are checked before they are sent to a client.
pub fn is_encoded(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Saturation and lightness of an sRGB colour (HSL), each `0.0..=1.0`.
fn saturation_lightness(r: u8, g: u8, b: u8) -> (f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    if (max - min).abs() < f32::EPSILON {
        return (0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    (s, l)
}

/// The colours of RGBA pixel data. `None` when no pixel is opaque enough
/// to count.
pub fn tone_from_rgba(data: &[u8]) -> Option<CoverTone> {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    let mut best: Option<[u8; 3]> = None;
    let mut best_score = -1.0f32;
    for &[pr, pg, pb, alpha] in data.as_chunks::<4>().0 {
        if alpha < MIN_ALPHA {
            continue;
        }
        r += u64::from(pr);
        g += u64::from(pg);
        b += u64::from(pb);
        n += 1;
        let (s, l) = saturation_lightness(pr, pg, pb);
        // Strong colour at a usable brightness; near-black and near-white
        // pixels (borders, logos, paper) score low.
        let score = s * (1.0 - (l - 0.5).abs() * 1.6);
        if score > best_score {
            best_score = score;
            best = Some([pr, pg, pb]);
        }
    }
    if n == 0 {
        return None;
    }
    // Rounded to nearest, like the browser's `Math.round`.
    let mean = |sum: u64| u8::try_from((sum + n / 2) / n).unwrap_or(u8::MAX);
    let average = [mean(r), mean(g), mean(b)];
    let vibrant = match best {
        Some(c) if best_score > MIN_VIBRANT_SCORE => c,
        _ => average,
    };
    Some(CoverTone { average, vibrant })
}

/// The colours of a decoded cover.
pub fn tone_from_image(img: &DynamicImage) -> Option<CoverTone> {
    let sample = img
        .resize_exact(SAMPLE_W, SAMPLE_H, FilterType::Triangle)
        .to_rgba8();
    tone_from_rgba(sample.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn solid(w: u32, h: u32, px: [u8; 4]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, Rgba(px)))
    }

    #[test]
    fn a_solid_cover_is_its_own_average_and_vibrant() {
        let tone = tone_from_image(&solid(60, 90, [200, 40, 30, 255])).unwrap();
        assert_eq!(tone.average, [200, 40, 30]);
        assert_eq!(tone.vibrant, [200, 40, 30]);
        assert_eq!(tone.encode(), "c8281ec8281e");
    }

    #[test]
    fn the_vibrant_colour_is_the_saturated_mid_bright_one() {
        // Mostly paper white, a strip of pure red, a strip of near-black.
        let mut data = Vec::new();
        for i in 0..100 {
            let px = match i {
                0..=9 => [230, 20, 20, 255],
                10..=19 => [8, 8, 8, 255],
                _ => [250, 250, 250, 255],
            };
            data.extend_from_slice(&px);
        }
        let tone = tone_from_rgba(&data).unwrap();
        assert_eq!(tone.vibrant, [230, 20, 20]);
        // 10 red, 10 near-black, 80 white.
        assert_eq!(tone.average, [224, 203, 203]);
    }

    #[test]
    fn a_grey_cover_has_no_vibrant_colour_of_its_own() {
        let mut data = Vec::new();
        for v in [40u8, 120, 200] {
            data.extend_from_slice(&[v, v, v, 255]);
        }
        let tone = tone_from_rgba(&data).unwrap();
        assert_eq!(tone.average, [120, 120, 120]);
        assert_eq!(tone.vibrant, tone.average);
    }

    #[test]
    fn transparent_pixels_do_not_count() {
        let data = [
            10, 200, 10, 255, // counts
            255, 0, 0, 100, // mostly transparent: skipped
        ];
        let tone = tone_from_rgba(&data).unwrap();
        assert_eq!(tone.average, [10, 200, 10]);
        assert!(tone_from_rgba(&[255, 0, 0, 0]).is_none());
        assert!(tone_from_image(&solid(4, 6, [1, 2, 3, 0])).is_none());
    }

    #[test]
    fn encoded_values_are_twelve_lowercase_hex_digits() {
        assert!(is_encoded("c8281ec8281e"));
        assert!(!is_encoded("C8281EC8281E"));
        assert!(!is_encoded("c8281e"));
        assert!(!is_encoded("c8281ec8281g"));
        assert!(!is_encoded("\"><script>x"));
    }
}
