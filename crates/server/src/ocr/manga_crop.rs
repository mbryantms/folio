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
//! - the requested rectangle as is;
//! - when the rectangle is one of the page's balloons (a tap on a
//!   region, or a drawn rectangle that snapped to one): the detector's
//!   text box inside it, if exactly one lies clearly inside, and the
//!   balloon otherwise.
//!
//! Two kinds of rectangle are never narrowed in the second mode:
//!
//! - **A rectangle that is not a region of the page**: one drawn by hand
//!   and not snapped (`detect` off, or nothing under it). It is what the
//!   user enclosed, and reading less than that would be silent.
//! - **A piece of free text** (class 2): it is not a balloon, and a text
//!   box lying in it belongs to a balloon drawn inside it. A free-text
//!   box that a balloon box overlaps almost entirely
//!   ([`NEAR_DUPLICATE_IOU`]) is the same balloon found twice, of which
//!   the regions kept the free-text one: it is treated as the balloon.
//!
//! [`MANGA_CROP`] names the mode the server uses: the text box. It was
//! chosen by a comparison, not against a reference text: on 17 pages of
//! one manga volume, 134 balloons and captions that both the previous
//! pipeline and this one have a box for, `manga-ocr` returned the same
//! string as the previous pipeline for 120 with the text box, for 113
//! with the balloon box as is, and for 116 with the balloon box inset by
//! 6%. `docs/dev/ocr.md` has the full counts and what they do not show.
//! The comparison tapped regions, which is the case the rule narrows.
//! [`manga_crop`] is a pure function of the mode, the rectangle and the
//! page's boxes, so a tool can run both.
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
//! detector run. A request whose detection fails is an error; nothing is
//! read without it, because a result read from the whole rectangle would
//! be cached under the key of the narrowed one.
//!
//! Only the current engine set uses this. With the legacy set a Japanese
//! request reads what it always read (the requested rectangle, or the
//! `comic-text-detector` box it was snapped to).

use super::EngineSet;
use super::bubbles::{CLASS_BUBBLE, CLASS_TEXT_BUBBLE};
use super::cache::CachedBbox;
use super::pipeline::{Rect, bbox_to_rect};
use super::regions::{self, NEAR_DUPLICATE_IOU};

/// What a Japanese request reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MangaCrop {
    /// The requested rectangle as is.
    Region,
    /// For a rectangle that is one of the page's balloons: the
    /// detector's text box (class 1) when exactly one lies clearly
    /// inside it, else the balloon. Any other rectangle as is.
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

/// What the server hands to `manga-ocr`: for a balloon of the page, the
/// text box when exactly one lies clearly inside it, else the balloon
/// (see the module docs for the comparison behind the choice). Changing
/// it rolls the result-cache keys by itself (it is part of the engine
/// fingerprint).
pub const MANGA_CROP: MangaCrop = MangaCrop::TextInRegion;

/// A text box lies inside the region when at least this share of its
/// area does.
pub const TEXT_INSIDE_MIN: f32 = 0.8;

/// A requested rectangle is one of the page's region boxes when each of
/// its edges is within this many pixels of the box's: a region goes to
/// the client in percent and comes back rounded to whole pixels.
pub const SAME_BOX_SLACK: u32 = 2;

/// The rectangle of the page a Japanese request reads.
///
/// `region` is the request's rectangle (after any snap), `boxes` the
/// page's detection (every class; empty when there is none), both in
/// page pixels of a `page_w × page_h` page.
///
/// [`MangaCrop::Region`] returns `region`. [`MangaCrop::TextInRegion`]
/// narrows only a rectangle that is one of the page's regions for the
/// current engines ([`regions::regions`], each edge within
/// [`SAME_BOX_SLACK`]) and a balloon: it returns the one class-1 box of
/// which at least [`TEXT_INSIDE_MIN`] lies in `region`, as detected (it
/// may reach a little past the region). A text box with a smaller share
/// inside belongs to a neighbour and is not counted. It returns `region`
/// when there is no such box or several, when `region` is not a region
/// of the page, and when it is a piece of free text (class 2) that no
/// balloon box overlaps by [`NEAR_DUPLICATE_IOU`] or more.
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
    let rect_of = |b: &CachedBbox| bbox_to_rect(b, page_w, page_h);
    // The region boxes of the page this rectangle is (normally one).
    let page_regions = regions::regions(EngineSet::Current, boxes, page_w, page_h);
    let mut own = page_regions
        .iter()
        .filter(|b| rect_of(b).is_some_and(|r| same_box(&r, &region)))
        .peekable();
    if own.peek().is_none() {
        // Not a region of the page: what the user enclosed.
        return region;
    }
    let balloon = own.any(|b| b.class == CLASS_BUBBLE)
        || boxes
            .iter()
            .filter(|b| b.class == CLASS_BUBBLE)
            .filter_map(rect_of)
            .any(|b| iou(&b, &region) >= NEAR_DUPLICATE_IOU);
    if !balloon {
        return region;
    }
    let mut inside = boxes
        .iter()
        .filter(|b| b.class == CLASS_TEXT_BUBBLE)
        .filter_map(rect_of)
        .filter(|text| {
            let shared = text.intersection(&region).map_or(0, |i| i.area());
            shared as f32 / text.area() as f32 >= TEXT_INSIDE_MIN
        });
    match (inside.next(), inside.next()) {
        (Some(only), None) => only,
        // None, or two texts in one balloon: not clear which is meant.
        _ => region,
    }
}

/// Whether two rectangles are the same box, edge for edge, to within
/// [`SAME_BOX_SLACK`].
fn same_box(a: &Rect, b: &Rect) -> bool {
    let near = |p: u32, q: u32| p.abs_diff(q) <= SAME_BOX_SLACK;
    near(a.x, b.x) && near(a.y, b.y) && near(a.x + a.w, b.x + b.w) && near(a.y + a.h, b.y + b.h)
}

/// Intersection over union of two rectangles.
fn iou(a: &Rect, b: &Rect) -> f32 {
    let shared = a.intersection(b).map_or(0, |i| i.area());
    let union = a.area() + b.area() - shared;
    if union == 0 {
        0.0
    } else {
        shared as f32 / union as f32
    }
}

/// The constants above that decide which box a Japanese request reads,
/// for the result-cache fingerprint (`ocr::cache`; the mode itself is
/// there as [`MangaCrop::label`], and the near-duplicate threshold with
/// the regions').
pub(crate) fn fingerprint() -> String {
    format!("manga-crop:inside={TEXT_INSIDE_MIN};same-box={SAME_BOX_SLACK}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::bubbles::CLASS_TEXT_FREE;

    const PAGE: (u32, u32) = (1000, 1500);

    /// A balloon at (100, 100), 200 wide and 300 tall.
    const BALLOON: Rect = Rect {
        x: 100,
        y: 100,
        w: 200,
        h: 300,
    };

    fn bbox(class: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> CachedBbox {
        scored(class, 0.9, x0, y0, x1, y1)
    }

    fn scored(class: u32, score: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> CachedBbox {
        CachedBbox {
            xmin: x0,
            ymin: y0,
            xmax: x1,
            ymax: y1,
            confidence: score,
            class,
        }
    }

    /// The detector's box for [`BALLOON`].
    fn balloon() -> CachedBbox {
        bbox(CLASS_BUBBLE, 100.0, 100.0, 300.0, 400.0)
    }

    fn text(x0: f32, y0: f32, x1: f32, y1: f32) -> CachedBbox {
        bbox(CLASS_TEXT_BUBBLE, x0, y0, x1, y1)
    }

    /// What a request for [`BALLOON`] reads on a page with the balloon
    /// and `others`.
    fn crop(mode: MangaCrop, others: &[CachedBbox]) -> Rect {
        let mut page = vec![balloon()];
        page.extend_from_slice(others);
        manga_crop(mode, BALLOON, &page, PAGE.0, PAGE.1)
    }

    /// What a request for `region` reads on a page with exactly `page`.
    fn crop_on(region: Rect, page: &[CachedBbox]) -> Rect {
        manga_crop(MangaCrop::TextInRegion, region, page, PAGE.0, PAGE.1)
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

    /// What the server runs: for a balloon, the text box when exactly
    /// one is clear, else the balloon.
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
    fn one_clear_text_box_inside_the_balloon_is_read_instead() {
        assert_eq!(crop(MangaCrop::TextInRegion, &[own_text()]), OWN_TEXT);
        // Other classes around it, and texts of other balloons, do not
        // matter.
        let others = [
            own_text(),
            bbox(CLASS_TEXT_FREE, 150.0, 150.0, 250.0, 200.0),
            bbox(CLASS_BUBBLE, 500.0, 100.0, 700.0, 400.0),
            text(540.0, 130.0, 660.0, 370.0),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &others), OWN_TEXT);
    }

    #[test]
    fn no_text_box_in_the_balloon_reads_the_balloon() {
        assert_eq!(crop(MangaCrop::TextInRegion, &[]), BALLOON);
        // Only balloons and free text detected: nothing to narrow to.
        let others = [bbox(CLASS_TEXT_FREE, 150.0, 150.0, 250.0, 200.0)];
        assert_eq!(crop(MangaCrop::TextInRegion, &others), BALLOON);
        // A text box elsewhere on the page.
        assert_eq!(
            crop(MangaCrop::TextInRegion, &[text(540.0, 130.0, 660.0, 370.0)]),
            BALLOON
        );
    }

    #[test]
    fn two_text_boxes_in_the_balloon_read_the_balloon() {
        let others = [
            text(120.0, 120.0, 190.0, 380.0),
            text(210.0, 120.0, 280.0, 380.0),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &others), BALLOON);
    }

    /// A text box counts as inside from 80% of its area.
    #[test]
    fn a_text_box_must_lie_mostly_inside_the_balloon() {
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

    /// A neighbour's text reaching into the balloon is not this
    /// balloon's: it neither is read nor makes the choice unclear.
    #[test]
    fn a_neighbours_text_reaching_into_the_balloon_is_not_counted() {
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

    /// Only a region of the page is narrowed. A rectangle drawn by hand
    /// is what the user enclosed, also when it holds exactly one text
    /// box.
    #[test]
    fn a_rectangle_that_is_not_a_region_of_the_page_is_read_as_sent() {
        // Around the balloon's text, inside the balloon.
        let drawn = Rect {
            x: 130,
            y: 120,
            w: 140,
            h: 260,
        };
        assert_eq!(crop_on(drawn, &[balloon(), own_text()]), drawn);
        // Around the whole balloon, three pixels wider on each side.
        let around = Rect {
            x: 97,
            y: 97,
            w: 206,
            h: 306,
        };
        assert_eq!(crop_on(around, &[balloon(), own_text()]), around);
        // The balloon's own rectangle on a page whose detection does
        // not hold that balloon (only its text was found).
        assert_eq!(crop_on(BALLOON, &[own_text()]), BALLOON);
        assert_eq!(crop_on(BALLOON, &[]), BALLOON);
    }

    /// A region goes to the client in percent and comes back rounded: up
    /// to two pixels off on an edge it is still that region.
    #[test]
    fn a_region_is_recognised_to_within_two_pixels_an_edge() {
        let page = [balloon(), own_text()];
        let near = Rect {
            x: 102,
            y: 98,
            w: 199,
            h: 304,
        };
        assert_eq!(crop_on(near, &page), OWN_TEXT);
        let three_off = Rect {
            x: 103,
            y: 100,
            w: 197,
            h: 300,
        };
        assert_eq!(crop_on(three_off, &page), three_off);
    }

    /// A piece of free text is read as is, also when a balloon drawn
    /// inside it puts exactly one text box in it.
    #[test]
    fn a_free_text_region_is_read_as_is() {
        let free_text = || bbox(CLASS_TEXT_FREE, 100.0, 100.0, 300.0, 400.0);
        let inner_balloon = || bbox(CLASS_BUBBLE, 130.0, 120.0, 270.0, 380.0);
        let page = [free_text(), inner_balloon(), own_text()];
        assert_eq!(crop_on(BALLOON, &page), BALLOON);
        // The balloon inside it is a region of its own and is narrowed.
        let inner = Rect {
            x: 130,
            y: 120,
            w: 140,
            h: 260,
        };
        assert_eq!(crop_on(inner, &page), OWN_TEXT);
        // A free-text box elsewhere does not make this balloon free
        // text.
        let elsewhere = [
            balloon(),
            bbox(CLASS_TEXT_FREE, 500.0, 100.0, 700.0, 400.0),
            own_text(),
        ];
        assert_eq!(crop_on(BALLOON, &elsewhere), OWN_TEXT);
    }

    /// One balloon found twice, as a balloon and as free text of a
    /// slightly different size, of which the regions kept the free-text
    /// box (the higher score): still a balloon.
    #[test]
    fn a_free_text_box_merged_with_a_balloon_is_a_balloon() {
        let as_free_text = scored(CLASS_TEXT_FREE, 0.95, 100.0, 100.0, 300.0, 400.0);
        // Ten pixels narrower and lower: not the same box edge for
        // edge, but well over the near-duplicate overlap.
        let as_balloon = scored(CLASS_BUBBLE, 0.80, 110.0, 110.0, 300.0, 400.0);
        let page = [as_free_text.clone(), as_balloon, own_text()];
        let kept = regions::regions(EngineSet::Current, &page, PAGE.0, PAGE.1);
        assert_eq!(
            kept.iter().map(|b| b.class).collect::<Vec<_>>(),
            [CLASS_TEXT_FREE],
            "the regions kept the free-text box of the pair"
        );
        assert_eq!(crop_on(BALLOON, &page), OWN_TEXT);
        // A balloon that only lies inside the free text overlaps it far
        // less: the free text stays free text.
        let inside = scored(CLASS_BUBBLE, 0.80, 130.0, 120.0, 270.0, 380.0);
        assert_eq!(
            crop_on(BALLOON, &[as_free_text, inside, own_text()]),
            BALLOON
        );
    }

    #[test]
    fn boxes_that_are_not_rectangles_on_the_page_are_ignored() {
        let others = [
            // Collapsed, and wholly off the page.
            text(150.0, 150.0, 150.0, 300.0),
            text(1200.0, 100.0, 1300.0, 200.0),
            own_text(),
        ];
        assert_eq!(crop(MangaCrop::TextInRegion, &others), OWN_TEXT);
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
        let page = [
            bbox(CLASS_BUBBLE, 900.0, 1300.0, 1000.0, 1500.0),
            text(920.4, 1320.6, 1010.0, 1510.0),
        ];
        assert_eq!(
            crop_on(region, &page),
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
