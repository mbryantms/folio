//! Which pixels a Japanese request hands to `manga-ocr`.
//!
//! With the current engine set a region is a balloon box (or a piece of
//! free text). `manga-ocr` was trained on tight crops of text, and the
//! previous detector handed it a text block, not a balloon; the balloon
//! box holds the text plus the white around it and the balloon's outline.
//! The page detector also returns the text inside each balloon (class 1,
//! `text_bubble`), kept in the page's detection for this purpose. So
//! there are two candidates for what to read ([`MangaCrop`]):
//!
//! - the requested region as is;
//! - the detector's text box inside the region, when there is exactly one
//!   clear one, and the region otherwise.
//!
//! **Which one reads Japanese better has not been measured.**
//! [`MANGA_CROP`] names the one the server uses, and is the region as is
//! until a measurement decides. [`manga_crop`] is a pure function of the
//! mode, the region and the page's boxes, so a tool can run both.
//!
//! Only the current engine set uses this. With the legacy set a Japanese
//! request reads what it always read (the requested rectangle, or the
//! `comic-text-detector` box it was snapped to).

use super::bubbles::CLASS_TEXT_BUBBLE;
use super::cache::CachedBbox;
use super::pipeline::{Rect, bbox_to_rect};

/// What a Japanese request reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MangaCrop {
    /// The requested region as is (the balloon box).
    Region,
    /// The detector's text box (class 1) lying inside the requested
    /// region when there is exactly one clear one; else the region.
    TextInRegion,
}

impl MangaCrop {
    /// Stable name, part of the result-cache fingerprint.
    pub const fn label(self) -> &'static str {
        match self {
            MangaCrop::Region => "region",
            MangaCrop::TextInRegion => "text-in-region",
        }
    }
}

/// What the server hands to `manga-ocr`. **Undecided**: the region as is
/// until the two behaviours have been measured on Japanese pages.
/// Changing it rolls the result-cache keys by itself (it is part of the
/// engine fingerprint).
pub const MANGA_CROP: MangaCrop = MangaCrop::Region;

/// A text box lies inside the region when at least this share of its
/// area does.
pub const TEXT_INSIDE_MIN: f32 = 0.8;

/// A second text box with more than this share of its area in the
/// region makes the choice unclear (the region holds part of another
/// text), and the region is read as is.
pub const TEXT_STRAY_MAX: f32 = 0.2;

/// The rectangle of the page a Japanese request reads.
///
/// `region` is the request's rectangle (after any snap), `boxes` the
/// page's detection (every class; empty when there is none), both in
/// page pixels of a `page_w × page_h` page.
///
/// [`MangaCrop::Region`] returns `region`. [`MangaCrop::TextInRegion`]
/// returns the one class-1 box of which at least [`TEXT_INSIDE_MIN`]
/// lies in `region`, as detected (it may reach a little past the
/// region), provided no other class-1 box has more than
/// [`TEXT_STRAY_MAX`] of its area in `region`; with no such box, several,
/// or a stray one, it returns `region`.
pub fn manga_crop(
    mode: MangaCrop,
    region: Rect,
    boxes: &[CachedBbox],
    page_w: u32,
    page_h: u32,
) -> Rect {
    if mode == MangaCrop::Region {
        return region;
    }
    let mut inside: Option<Rect> = None;
    for text in boxes
        .iter()
        .filter(|b| b.class == CLASS_TEXT_BUBBLE)
        .filter_map(|b| bbox_to_rect(b, page_w, page_h))
    {
        let shared = text.intersection(&region).map_or(0, |i| i.area());
        let share = shared as f32 / text.area() as f32;
        if share >= TEXT_INSIDE_MIN {
            if inside.is_some() {
                // Two texts in one region: not clear which is meant.
                return region;
            }
            inside = Some(text);
        } else if share > TEXT_STRAY_MAX {
            return region;
        }
    }
    inside.unwrap_or(region)
}

/// The constants above that decide which box a Japanese request reads,
/// for the result-cache fingerprint (`ocr::cache`; the mode itself is
/// there as [`MangaCrop::label`]).
pub(crate) fn fingerprint() -> String {
    format!("manga-crop:inside={TEXT_INSIDE_MIN};stray={TEXT_STRAY_MAX}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::bubbles::{CLASS_BUBBLE, CLASS_TEXT_FREE};

    const PAGE: (u32, u32) = (1000, 1500);

    /// A balloon at (100, 100), 200 wide and 300 tall.
    const BALLOON: Rect = Rect {
        x: 100,
        y: 100,
        w: 200,
        h: 300,
    };

    fn bbox(class: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> CachedBbox {
        CachedBbox {
            xmin: x0,
            ymin: y0,
            xmax: x1,
            ymax: y1,
            confidence: 0.9,
            class,
        }
    }

    fn text(x0: f32, y0: f32, x1: f32, y1: f32) -> CachedBbox {
        bbox(CLASS_TEXT_BUBBLE, x0, y0, x1, y1)
    }

    fn crop(mode: MangaCrop, boxes: &[CachedBbox]) -> Rect {
        manga_crop(mode, BALLOON, boxes, PAGE.0, PAGE.1)
    }

    /// The text of the balloon, well inside it.
    fn own_text() -> CachedBbox {
        text(140.0, 130.0, 260.0, 370.0)
    }

    const OWN_TEXT: Rect = Rect {
        x: 140,
        y: 130,
        w: 120,
        h: 240,
    };

    #[test]
    fn the_region_mode_reads_the_region_whatever_was_detected() {
        assert_eq!(crop(MangaCrop::Region, &[]), BALLOON);
        assert_eq!(crop(MangaCrop::Region, &[own_text()]), BALLOON);
    }

    #[test]
    fn the_server_reads_the_region_until_measured() {
        assert_eq!(MANGA_CROP, MangaCrop::Region);
    }

    #[test]
    fn one_clear_text_box_inside_the_region_is_read_instead() {
        assert_eq!(crop(MangaCrop::TextInRegion, &[own_text()]), OWN_TEXT);
        // Other classes around it, and texts of other balloons, do not
        // matter.
        let page = [
            bbox(CLASS_BUBBLE, 100.0, 100.0, 300.0, 400.0),
            own_text(),
            bbox(CLASS_TEXT_FREE, 150.0, 150.0, 250.0, 200.0),
            bbox(CLASS_BUBBLE, 500.0, 100.0, 700.0, 400.0),
            text(540.0, 130.0, 660.0, 370.0),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &page), OWN_TEXT);
    }

    #[test]
    fn no_text_box_in_the_region_reads_the_region() {
        assert_eq!(crop(MangaCrop::TextInRegion, &[]), BALLOON);
        // Only balloons and free text detected: nothing to narrow to.
        let page = [
            bbox(CLASS_BUBBLE, 100.0, 100.0, 300.0, 400.0),
            bbox(CLASS_TEXT_FREE, 150.0, 150.0, 250.0, 200.0),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &page), BALLOON);
        // A text box elsewhere on the page.
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[text(540.0, 130.0, 660.0, 370.0)]),
            BALLOON
        );
    }

    #[test]
    fn two_text_boxes_in_the_region_read_the_region() {
        let page = [
            text(120.0, 120.0, 190.0, 380.0),
            text(210.0, 120.0, 280.0, 380.0),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &page), BALLOON);
    }

    /// A text box counts as inside from 80% of its area.
    #[test]
    fn a_text_box_must_lie_mostly_inside_the_region() {
        // 100 wide, 80 of it inside the balloon's right edge (x = 300).
        let mostly_in = text(220.0, 200.0, 320.0, 300.0);
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[mostly_in]),
            Rect {
                x: 220,
                y: 200,
                w: 100,
                h: 100
            },
            "read as detected, reaching past the region"
        );
        // 79 of 100 inside: not this balloon's text.
        let half_out = text(221.0, 200.0, 321.0, 300.0);
        assert_eq!(crop(MangaCrop::TextInRegion, &[half_out]), BALLOON);
    }

    /// A neighbour's text reaching into the region makes it unclear; one
    /// that only grazes it does not.
    #[test]
    fn a_stray_text_box_in_the_region_reads_the_region() {
        // 100 wide, 21 of it inside the balloon: more than a fifth.
        let stray = || text(279.0, 200.0, 379.0, 300.0);
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[own_text(), stray()]),
            BALLOON
        );
        // Order does not matter.
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[stray(), own_text()]),
            BALLOON
        );
        // 20 of 100 inside: a graze, ignored.
        let graze = text(280.0, 200.0, 380.0, 300.0);
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[own_text(), graze]),
            OWN_TEXT
        );
        // A stray box alone gives nothing to read instead.
        assert_eq!(crop(MangaCrop::TextInRegion, &[stray()]), BALLOON);
    }

    #[test]
    fn boxes_that_are_not_rectangles_on_the_page_are_ignored() {
        let page = [
            // Collapsed, and wholly off the page.
            text(150.0, 150.0, 150.0, 300.0),
            text(1200.0, 100.0, 1300.0, 200.0),
            own_text(),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &page), OWN_TEXT);
    }

    /// The text box is rounded and clamped to the page like every other
    /// crop.
    #[test]
    fn the_text_box_is_a_whole_pixel_rectangle_on_the_page() {
        let region = Rect {
            x: 900,
            y: 1300,
            w: 100,
            h: 200,
        };
        let got = manga_crop(
            MangaCrop::TextInRegion,
            region,
            &[text(920.4, 1320.6, 1010.0, 1510.0)],
            PAGE.0,
            PAGE.1,
        );
        assert_eq!(
            got,
            Rect {
                x: 920,
                y: 1321,
                w: 80,
                h: 179
            }
        );
    }

    #[test]
    fn the_modes_have_distinct_fingerprint_labels() {
        assert_ne!(MangaCrop::Region.label(), MangaCrop::TextInRegion.label());
    }
}
