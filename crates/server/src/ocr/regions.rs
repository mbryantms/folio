//! The regions of a page: which boxes of a detection are offered to
//! clients (the text-regions route) and, with the current engines, which
//! a drawn rectangle snaps to.
//!
//! A page's detection is cached raw, every box of every class. The
//! server turns it into the final set here, so the client draws, taps
//! and reads exactly the boxes it is sent and never drops one itself.
//! What "the same thing twice" means depends on the detector:
//!
//! - **Current engines** ([`super::bubbles`]): a region is a balloon or a
//!   piece of free text. A box inside another is a different thing (a
//!   balloon inside a large free-text box, a balloon inside a balloon the
//!   detector also boxed with its neighbour), so containment drops
//!   nothing. The detector does sometimes return one region twice, as a
//!   balloon and as free text a few pixels apart: two region boxes whose
//!   intersection over union is at least [`NEAR_DUPLICATE_IOU`] are one
//!   region, and the higher score stays (on a tie the larger box).
//! - **Legacy engines** (`comic-text-detector`): the boxes are text
//!   blocks and the text lines inside them, so a box mostly inside a
//!   larger one is the same text again. A box with at least
//!   [`LEGACY_NESTED_OVERLAP`] of its area inside a box at least as large
//!   is dropped. This is the rule the reader applied itself before the
//!   server did, moved here unchanged.
//!
//! Both rules work on the boxes clamped to the page and keep detector
//! order.

use super::EngineSet;
use super::cache::CachedBbox;

/// Current engines: two region boxes with at least this intersection
/// over union are the same region found twice.
pub const NEAR_DUPLICATE_IOU: f32 = 0.8;

/// Legacy engines: a box with at least this share of its area inside a
/// box at least as large is a nested duplicate.
pub const LEGACY_NESTED_OVERLAP: f32 = 0.8;

/// The regions of a `page_w × page_h` page's detection for an engine
/// set: the boxes of a region class ([`EngineSet::is_region_class`]),
/// clamped to the page (a box with nothing left on it is dropped), with
/// the set's duplicates removed, in detector order.
pub fn regions(
    engines: EngineSet,
    boxes: &[CachedBbox],
    page_w: u32,
    page_h: u32,
) -> Vec<CachedBbox> {
    let on_page: Vec<CachedBbox> = boxes
        .iter()
        .filter(|b| engines.is_region_class(b.class))
        .filter_map(|b| clamp_to_page(b, page_w, page_h))
        .collect();
    let keep = match engines {
        EngineSet::Current => without_near_duplicates(&on_page),
        EngineSet::Legacy => without_nested(&on_page),
    };
    on_page
        .into_iter()
        .zip(keep)
        .filter_map(|(b, keep)| keep.then_some(b))
        .collect()
}

/// `b` cut to the page; `None` when a coordinate is not finite or no
/// area is left.
fn clamp_to_page(b: &CachedBbox, page_w: u32, page_h: u32) -> Option<CachedBbox> {
    if ![b.xmin, b.ymin, b.xmax, b.ymax]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    let (w, h) = (page_w as f32, page_h as f32);
    let clamped = CachedBbox {
        xmin: b.xmin.clamp(0.0, w),
        ymin: b.ymin.clamp(0.0, h),
        xmax: b.xmax.clamp(0.0, w),
        ymax: b.ymax.clamp(0.0, h),
        ..b.clone()
    };
    (clamped.xmax > clamped.xmin && clamped.ymax > clamped.ymin).then_some(clamped)
}

fn area(b: &CachedBbox) -> f32 {
    (b.xmax - b.xmin).max(0.0) * (b.ymax - b.ymin).max(0.0)
}

fn intersection(a: &CachedBbox, b: &CachedBbox) -> f32 {
    let w = a.xmax.min(b.xmax) - a.xmin.max(b.xmin);
    let h = a.ymax.min(b.ymax) - a.ymin.max(b.ymin);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// Intersection over union of two boxes with area.
fn iou(a: &CachedBbox, b: &CachedBbox) -> f32 {
    let shared = intersection(a, b);
    let union = area(a) + area(b) - shared;
    if union > 0.0 { shared / union } else { 0.0 }
}

/// Which boxes stay under the current engines' rule. Boxes are taken
/// best first (score, then area, then detector order); one is kept
/// unless it overlaps a box already kept by [`NEAR_DUPLICATE_IOU`] or
/// more.
fn without_near_duplicates(boxes: &[CachedBbox]) -> Vec<bool> {
    let mut best_first: Vec<usize> = (0..boxes.len()).collect();
    best_first.sort_by(|&a, &b| {
        boxes[b]
            .confidence
            .total_cmp(&boxes[a].confidence)
            .then(area(&boxes[b]).total_cmp(&area(&boxes[a])))
            .then(a.cmp(&b))
    });
    let mut keep = vec![false; boxes.len()];
    let mut kept: Vec<usize> = Vec::new();
    for i in best_first {
        if kept
            .iter()
            .all(|&k| iou(&boxes[k], &boxes[i]) < NEAR_DUPLICATE_IOU)
        {
            keep[i] = true;
            kept.push(i);
        }
    }
    keep
}

/// Which boxes stay under the legacy engines' rule: a box is dropped
/// when some other box that is larger (or as large and earlier) holds at
/// least [`LEGACY_NESTED_OVERLAP`] of its area. Every other box counts,
/// also one that is itself dropped, exactly as the reader's rule did.
fn without_nested(boxes: &[CachedBbox]) -> Vec<bool> {
    boxes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let a = area(b);
            !boxes.iter().enumerate().any(|(j, other)| {
                if i == j {
                    return false;
                }
                let other_area = area(other);
                // Only a strictly larger (or equal-size, earlier) box can
                // swallow this one: two identical boxes keep exactly one.
                if other_area < a || (other_area == a && j > i) {
                    return false;
                }
                intersection(b, other) / a >= LEGACY_NESTED_OVERLAP
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::bubbles::{CLASS_BUBBLE, CLASS_TEXT_BUBBLE, CLASS_TEXT_FREE};

    const PAGE: (u32, u32) = (1000, 1500);

    fn bbox(class: u32, score: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> CachedBbox {
        CachedBbox {
            xmin: x0,
            ymin: y0,
            xmax: x1,
            ymax: y1,
            confidence: score,
            class,
        }
    }

    fn rects(boxes: &[CachedBbox]) -> Vec<(f32, f32, f32, f32)> {
        boxes
            .iter()
            .map(|b| (b.xmin, b.ymin, b.xmax, b.ymax))
            .collect()
    }

    fn current(boxes: &[CachedBbox]) -> Vec<CachedBbox> {
        regions(EngineSet::Current, boxes, PAGE.0, PAGE.1)
    }

    fn legacy(boxes: &[CachedBbox]) -> Vec<CachedBbox> {
        regions(EngineSet::Legacy, boxes, PAGE.0, PAGE.1)
    }

    // ── Current engines ──

    /// A balloon inside a large free-text box is its own region: nothing
    /// is dropped for being contained, whichever of the two is larger.
    #[test]
    fn a_balloon_inside_a_free_text_box_is_kept() {
        let caption = bbox(CLASS_TEXT_FREE, 0.9, 100.0, 100.0, 700.0, 500.0);
        let balloon = bbox(CLASS_BUBBLE, 0.8, 200.0, 200.0, 400.0, 350.0);
        let got = current(&[caption.clone(), balloon.clone()]);
        assert_eq!(rects(&got), rects(&[caption, balloon]));
        // A balloon in a box the detector drew around it and its
        // neighbour: all three stay.
        let pair = bbox(CLASS_BUBBLE, 0.7, 100.0, 600.0, 600.0, 900.0);
        let left = bbox(CLASS_BUBBLE, 0.9, 110.0, 610.0, 340.0, 890.0);
        let right = bbox(CLASS_BUBBLE, 0.9, 360.0, 610.0, 590.0, 890.0);
        assert_eq!(current(&[pair, left, right]).len(), 3);
    }

    /// The same region boxed twice, a few pixels apart (here as free text
    /// and as a balloon): one stays, the higher score.
    #[test]
    fn two_near_identical_boxes_collapse_to_the_higher_score() {
        let as_text = bbox(CLASS_TEXT_FREE, 0.80, 399.0, 381.0, 746.0, 427.0);
        let as_balloon = bbox(CLASS_BUBBLE, 0.85, 400.0, 380.0, 747.0, 428.0);
        let other = bbox(CLASS_BUBBLE, 0.6, 50.0, 50.0, 200.0, 150.0);
        let got = current(&[as_text.clone(), other.clone(), as_balloon.clone()]);
        assert_eq!(rects(&got), rects(&[other.clone(), as_balloon.clone()]));
        assert_eq!(got[1].class, CLASS_BUBBLE);
        // Whatever the order they were detected in.
        let got = current(&[as_balloon.clone(), as_text, other.clone()]);
        assert_eq!(rects(&got), rects(&[as_balloon, other]));
    }

    #[test]
    fn equal_scores_keep_the_larger_box_then_the_earlier() {
        let small = bbox(CLASS_BUBBLE, 0.9, 101.0, 101.0, 299.0, 299.0);
        let large = bbox(CLASS_BUBBLE, 0.9, 100.0, 100.0, 300.0, 300.0);
        assert_eq!(
            rects(&current(&[small.clone(), large.clone()])),
            rects(std::slice::from_ref(&large))
        );
        // Identical boxes: exactly one, the first.
        let twin = bbox(CLASS_TEXT_FREE, 0.9, 100.0, 100.0, 300.0, 300.0);
        let got = current(&[large, twin]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].class, CLASS_BUBBLE);
    }

    /// The threshold itself counts as a duplicate; just under it does not.
    #[test]
    fn the_overlap_is_intersection_over_union_at_0_8() {
        let a = bbox(CLASS_BUBBLE, 0.9, 0.0, 0.0, 100.0, 100.0);
        // 100 x 80 inside `a`: IoU = 8000 / 10000.
        let at = bbox(CLASS_BUBBLE, 0.8, 0.0, 0.0, 100.0, 80.0);
        assert_eq!(current(&[a.clone(), at]).len(), 1);
        // 100 x 79: IoU 0.79.
        let under = bbox(CLASS_BUBBLE, 0.8, 0.0, 0.0, 100.0, 79.0);
        assert_eq!(current(&[a.clone(), under]).len(), 2);
        // Half of a small box hanging out of a large one: far from it.
        let overlapping = bbox(CLASS_BUBBLE, 0.8, 50.0, 0.0, 150.0, 100.0);
        assert_eq!(current(&[a, overlapping]).len(), 2);
    }

    /// A chain is resolved best first: the best box, then every box that
    /// is not a near-duplicate of one already kept.
    #[test]
    fn a_chain_of_near_duplicates_keeps_the_best_and_what_differs_from_it() {
        let a = bbox(CLASS_BUBBLE, 0.9, 0.0, 0.0, 100.0, 100.0);
        let b = bbox(CLASS_BUBBLE, 0.8, 0.0, 0.0, 100.0, 85.0);
        let c = bbox(CLASS_BUBBLE, 0.7, 0.0, 0.0, 100.0, 70.0);
        // a~b (0.85), b~c (0.82), a and c are not (0.70).
        assert_eq!(rects(&current(&[c.clone(), b, a.clone()])), rects(&[c, a]));
    }

    #[test]
    fn text_inside_balloons_is_never_a_region_of_the_current_engines() {
        let balloon = bbox(CLASS_BUBBLE, 0.9, 100.0, 100.0, 300.0, 400.0);
        let text = bbox(CLASS_TEXT_BUBBLE, 0.95, 140.0, 130.0, 260.0, 370.0);
        let got = current(&[balloon, text]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].class, CLASS_BUBBLE);
        // And it cannot take a region with it: a text box right on a
        // balloon does not make the balloon a duplicate.
        let balloon = bbox(CLASS_BUBBLE, 0.6, 100.0, 100.0, 300.0, 400.0);
        let same_as_text = bbox(CLASS_TEXT_BUBBLE, 0.99, 100.0, 100.0, 300.0, 400.0);
        assert_eq!(current(&[same_as_text, balloon]).len(), 1);
    }

    // ── Legacy engines ──

    /// A line inside its block is the same text again: the smaller box
    /// goes, whatever its class or score.
    #[test]
    fn a_legacy_nested_pair_drops_the_smaller() {
        let block = bbox(0, 0.6, 300.0, 300.0, 700.0, 700.0);
        let line = bbox(1, 0.99, 350.0, 450.0, 650.0, 530.0);
        let other = bbox(0, 0.9, 50.0, 800.0, 150.0, 900.0);
        let got = legacy(&[line.clone(), block.clone(), other.clone()]);
        assert_eq!(rects(&got), rects(&[block.clone(), other]));
        // The same pair is two regions for the current engines (given
        // region classes).
        let inner = bbox(CLASS_BUBBLE, 0.99, 350.0, 450.0, 650.0, 530.0);
        assert_eq!(current(&[inner, block]).len(), 2);
    }

    /// 80% of the smaller box's area inside the larger one is the line;
    /// less than that is a neighbour that overlaps.
    #[test]
    fn the_legacy_rule_is_a_share_of_the_smaller_box() {
        let block = bbox(0, 0.9, 0.0, 0.0, 400.0, 400.0);
        // 100 wide, 80 of it inside the block.
        let mostly_in = bbox(1, 0.9, 320.0, 100.0, 420.0, 200.0);
        assert_eq!(legacy(&[block.clone(), mostly_in]).len(), 1);
        // 79 of 100 inside.
        let neighbour = bbox(1, 0.9, 321.0, 100.0, 421.0, 200.0);
        assert_eq!(legacy(&[block, neighbour]).len(), 2);
    }

    #[test]
    fn legacy_identical_boxes_keep_the_first_only() {
        let a = bbox(0, 0.5, 100.0, 100.0, 300.0, 200.0);
        let twin = bbox(1, 0.9, 100.0, 100.0, 300.0, 200.0);
        let got = legacy(&[a, twin]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].class, 0);
    }

    /// As the reader's rule did, a box is dropped for sitting in any
    /// larger box, also one that is itself dropped.
    #[test]
    fn the_legacy_rule_counts_boxes_that_are_themselves_dropped() {
        let outer = bbox(0, 0.9, 0.0, 0.0, 500.0, 500.0);
        let middle = bbox(0, 0.9, 50.0, 50.0, 450.0, 450.0);
        let inner = bbox(1, 0.9, 100.0, 100.0, 400.0, 400.0);
        assert_eq!(
            rects(&legacy(&[inner, middle, outer.clone()])),
            rects(&[outer])
        );
    }

    // ── Both ──

    #[test]
    fn regions_are_clamped_to_the_page_and_empty_ones_dropped() {
        for engines in [EngineSet::Current, EngineSet::Legacy] {
            let got = regions(
                engines,
                &[
                    bbox(0, 0.9, -20.0, -5.0, 300.0, 200.0),
                    // Wholly off the page, collapsed, not finite.
                    bbox(0, 0.9, 1200.0, 100.0, 1300.0, 200.0),
                    bbox(0, 0.9, 500.0, 500.0, 500.0, 900.0),
                    bbox(0, 0.9, f32::NAN, 0.0, 10.0, 10.0),
                    bbox(0, 0.9, 900.0, 1400.0, 1100.0, 1600.0),
                ],
                PAGE.0,
                PAGE.1,
            );
            assert_eq!(
                rects(&got),
                [(0.0, 0.0, 300.0, 200.0), (900.0, 1400.0, 1000.0, 1500.0)],
                "{engines:?}"
            );
        }
        assert!(current(&[]).is_empty() && legacy(&[]).is_empty());
    }
}
