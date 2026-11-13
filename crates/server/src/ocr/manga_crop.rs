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
//! - the detector's text box inside the region, when exactly one lies
//!   clearly inside it, and the region otherwise. A region that is a
//!   piece of free text (class 2) is always read as is: it is not a
//!   balloon, and a text box lying in it belongs to a balloon drawn
//!   inside it.
//!
//! [`MANGA_CROP`] names the one the server uses: the text box. It was
//! chosen by a comparison, not against a reference text: on 17 pages of
//! one manga volume, 134 balloons and captions that both the previous
//! pipeline and this one have a box for, `manga-ocr` returned the same
//! string as the previous pipeline for 120 with the text box, for 113
//! with the balloon box as is, and for 116 with the balloon box inset by
//! 6%. `docs/dev/ocr.md` has the full counts and what they do not show.
//! The rule here is the one that comparison used. [`manga_crop`] is a
//! pure function of the mode, the region and the page's boxes, so a tool
//! can run both.
//!
//! Two known weaknesses of the text-box mode:
//!
//! - **A balloon with no text box.** The region is read as is. For a
//!   tiny balloon that holds only a mark the recogniser, given the
//!   balloon, invents characters.
//! - **A balloon with several lobes.** The detector may box the text of
//!   the larger lobe only, or box the other lobe's text so that it lies
//!   partly outside the balloon's box; the larger lobe's box is then the
//!   single one inside and the other lobe's text is not read.
//!
//! In this mode a Japanese request always needs the page's detection,
//! also for a tap on a region (`pipeline::uses_detection`): it comes
//! from the detect cache the text-regions route filled, else from one
//! detector run.
//!
//! Only the current engine set uses this. With the legacy set a Japanese
//! request reads what it always read (the requested rectangle, or the
//! `comic-text-detector` box it was snapped to).

use super::bubbles::{CLASS_BUBBLE, CLASS_TEXT_BUBBLE, CLASS_TEXT_FREE};
use super::cache::CachedBbox;
use super::pipeline::{Rect, bbox_to_rect};

/// What a Japanese request reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MangaCrop {
    /// The requested region as is (the balloon box).
    Region,
    /// The detector's text box (class 1) when exactly one lies clearly
    /// inside the requested region; else the region. A free-text region
    /// is read as is.
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

/// What the server hands to `manga-ocr`: the text box when exactly one
/// lies clearly inside the region, else the region (see the module docs
/// for the comparison behind the choice). Changing it rolls the
/// result-cache keys by itself (it is part of the engine fingerprint).
pub const MANGA_CROP: MangaCrop = MangaCrop::TextInRegion;

/// A text box lies inside the region when at least this share of its
/// area does.
pub const TEXT_INSIDE_MIN: f32 = 0.8;

/// A requested region is one of the page's boxes when each of its edges
/// is within this many pixels of the box's: a region goes to the client
/// in percent and comes back rounded to whole pixels.
pub const SAME_BOX_SLACK: u32 = 2;

/// The rectangle of the page a Japanese request reads.
///
/// `region` is the request's rectangle (after any snap), `boxes` the
/// page's detection (every class; empty when there is none), both in
/// page pixels of a `page_w × page_h` page.
///
/// [`MangaCrop::Region`] returns `region`. [`MangaCrop::TextInRegion`]
/// returns the one class-1 box of which at least [`TEXT_INSIDE_MIN`]
/// lies in `region`, as detected (it may reach a little past the
/// region). With no such box or several it returns `region`, and so it
/// does for a region that is one of the page's free-text boxes (and not
/// also a balloon). A text box with a smaller share inside the region
/// belongs to a neighbour and is not counted.
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
    let is_box_of = |class: u32| {
        boxes
            .iter()
            .filter(|b| b.class == class)
            .filter_map(|b| bbox_to_rect(b, page_w, page_h))
            .any(|r| same_box(&r, &region))
    };
    if is_box_of(CLASS_TEXT_FREE) && !is_box_of(CLASS_BUBBLE) {
        return region;
    }
    let mut inside = boxes
        .iter()
        .filter(|b| b.class == CLASS_TEXT_BUBBLE)
        .filter_map(|b| bbox_to_rect(b, page_w, page_h))
        .filter(|text| {
            let shared = text.intersection(&region).map_or(0, |i| i.area());
            shared as f32 / text.area() as f32 >= TEXT_INSIDE_MIN
        });
    match (inside.next(), inside.next()) {
        (Some(only), None) => only,
        // None, or two texts in one region: not clear which is meant.
        _ => region,
    }
}

/// Whether two rectangles are the same box, edge for edge, to within
/// [`SAME_BOX_SLACK`].
fn same_box(a: &Rect, b: &Rect) -> bool {
    let near = |p: u32, q: u32| p.abs_diff(q) <= SAME_BOX_SLACK;
    near(a.x, b.x) && near(a.y, b.y) && near(a.x + a.w, b.x + b.w) && near(a.y + a.h, b.y + b.h)
}

/// The constants above that decide which box a Japanese request reads,
/// for the result-cache fingerprint (`ocr::cache`; the mode itself is
/// there as [`MangaCrop::label`]).
pub(crate) fn fingerprint() -> String {
    format!("manga-crop:inside={TEXT_INSIDE_MIN};same-box={SAME_BOX_SLACK}")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// What the server runs: the text box when exactly one is clear,
    /// else the region.
    #[test]
    fn the_server_reads_the_text_box_when_exactly_one_is_clear() {
        assert_eq!(MANGA_CROP, MangaCrop::TextInRegion);
        assert_eq!(crop(MANGA_CROP, &[own_text()]), OWN_TEXT);
        assert_eq!(crop(MANGA_CROP, &[]), BALLOON);
        let two = [
            text(120.0, 120.0, 190.0, 380.0),
            text(210.0, 120.0, 280.0, 380.0),
        ];
        assert_eq!(crop(MANGA_CROP, &two), BALLOON);
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

    /// A neighbour's text reaching into the region is not this
    /// balloon's: it neither is read nor makes the choice unclear.
    #[test]
    fn a_neighbours_text_reaching_into_the_region_is_not_counted() {
        // 100 wide, half of it inside the balloon's right edge.
        let neighbour = || text(250.0, 200.0, 350.0, 300.0);
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[own_text(), neighbour()]),
            OWN_TEXT
        );
        // Order does not matter.
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[neighbour(), own_text()]),
            OWN_TEXT
        );
        // Alone it gives nothing to read instead.
        assert_eq!(crop(MangaCrop::TextInRegion, &[neighbour()]), BALLOON);
    }

    /// A piece of free text is read as is, also when a balloon drawn
    /// inside it puts exactly one text box in it.
    #[test]
    fn a_free_text_region_is_read_as_is() {
        let free_text = bbox(CLASS_TEXT_FREE, 100.0, 100.0, 300.0, 400.0);
        let inner = [bbox(CLASS_BUBBLE, 130.0, 120.0, 270.0, 380.0), own_text()];
        let page = [free_text.clone(), inner[0].clone(), inner[1].clone()];
        assert_eq!(crop(MangaCrop::TextInRegion, &page), BALLOON);
        // The region comes back from the client rounded: a pixel off on
        // an edge is still that box.
        let rounded = Rect {
            x: 101,
            y: 99,
            w: 200,
            h: 302,
        };
        assert_eq!(
            manga_crop(MangaCrop::TextInRegion, rounded, &page, PAGE.0, PAGE.1),
            rounded
        );
        // The same rectangle drawn by hand on a page with no such
        // free-text box is narrowed to the one text box in it.
        assert_eq!(crop(MangaCrop::TextInRegion, &inner), OWN_TEXT);
        // A free-text box elsewhere, or one of another size around the
        // same text, does not make this region free text.
        let other = [
            bbox(CLASS_TEXT_FREE, 90.0, 100.0, 300.0, 400.0),
            bbox(CLASS_TEXT_FREE, 500.0, 100.0, 700.0, 400.0),
            own_text(),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &other), OWN_TEXT);
        // Found both as a balloon and as free text: a balloon.
        let both = [
            free_text,
            bbox(CLASS_BUBBLE, 101.0, 100.0, 300.0, 401.0),
            own_text(),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &both), OWN_TEXT);
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
