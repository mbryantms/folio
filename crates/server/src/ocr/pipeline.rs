//! `detect → snap polygon → crop → recognize` orchestration.
//!
//! Callers hand us the **full page** image and the user's drag rect in
//! page-pixel coordinates. We:
//!
//!  1. Crop to the user's rect with a small margin so the detector
//!     has a little context around the bubble.
//!  2. Run `comic-text-detector` over that crop; pick the bbox whose
//!     intersection with the user's rect (translated into local
//!     coordinates) is largest.
//!  3. Crop to that bbox if one was found above the confidence
//!     threshold, else fall back to the user's rect verbatim.
//!  4. Hand the final image to the selected recognizer.
//!
//! All heavy work (detector inference + recognize) runs inside a
//! single [`tokio::task::spawn_blocking`] so the reactor isn't
//! stalled. We hold the detector + recognizer singletons by
//! `&'static` reference, which is `Send + Sync` thanks to the
//! [`Recognizer: Send + Sync`][crate::ocr::recognizer::Recognizer]
//! bound.

use image::DynamicImage;

use super::detector::Detector;
use super::recognizer::{Language, Recognition, Recognizer, manga::MangaOcr, western::WesternOcr};

/// Pixel rectangle in page coordinates. All four fields are
/// unsigned because callers must reject negatives at the API
/// boundary — the pipeline trusts its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let xmax = (self.x + self.w).min(other.x + other.w);
        let ymax = (self.y + self.h).min(other.y + other.h);
        if xmax > x && ymax > y {
            Some(Rect {
                x,
                y,
                w: xmax - x,
                h: ymax - y,
            })
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct OcrInput {
    pub page_image: DynamicImage,
    pub region: Rect,
    pub language: Language,
}

#[derive(Debug)]
pub struct OcrOutput {
    pub recognition: Recognition,
    /// Detector's snap-to-bubble rectangle in page-pixel coordinates,
    /// `None` if no bbox overlapped the user's region above
    /// confidence threshold.
    pub refined_bbox: Option<Rect>,
}

/// Detector confidence threshold below which a candidate bbox is
/// discarded. 0.5 matches dmMaze's default.
const DETECTOR_CONF: f32 = 0.5;
/// Non-max-suppression IoU threshold.
const DETECTOR_NMS: f32 = 0.5;
/// Padding added around the user's rect before running the detector
/// — gives the YOLO head a little context so it can see the full
/// bubble outline. Capped per-side at 1/4 of the region's larger
/// dimension so small drag rects don't blow up.
fn margin_for(region: &Rect) -> u32 {
    let largest = region.w.max(region.h);
    (largest / 4).clamp(8, 64)
}

/// Driver: returns the recognized text + a refined bubble outline.
///
/// Looks up both singletons before entering the blocking task: the
/// `OnceCell`s wrap the async HF download, which we can't run inside
/// `spawn_blocking`. The heavy CPU work then runs once both are
/// warm.
///
/// Records two Prometheus histograms (units: seconds, matching
/// `comic_reader_next_up_latency_seconds` and friends):
///
/// - `comic_ocr_pipeline_seconds` — wall-clock for the whole call,
///   tagged `lang="western"|"manga"`. Spans detector + recognize.
/// - `comic_ocr_recognize_seconds` — recognize-only time (recorded
///   inside [`run_blocking`]). Tagged the same way.
///
/// Use the gap between them to spot detector-bound vs recognizer-
/// bound deployments.
pub async fn run_ocr(input: OcrInput) -> anyhow::Result<OcrOutput> {
    let lang_label = match input.language {
        Language::Western => "western",
        Language::Manga => "manga",
    };
    let start = std::time::Instant::now();
    let detector = Detector::shared().await?;
    // The two recognizer impls share the trait; we hand the
    // blocking closure a `&'static dyn Recognizer` so the dispatch
    // table is resolved up front.
    let recognizer: &'static dyn Recognizer = match input.language {
        Language::Western => WesternOcr::shared().await?,
        Language::Manga => MangaOcr::shared().await?,
    };

    let result =
        tokio::task::spawn_blocking(move || run_blocking(detector, recognizer, input, lang_label))
            .await
            .map_err(|e| anyhow::anyhow!("ocr task panicked: {e}"))?;
    // Record pipeline wall time regardless of success: an operator
    // watching this dashboard cares about both latency *and* error
    // rate. Failed paths are still tagged on `lang`; success is
    // implied by the dual `ocr_failed`-counter elsewhere.
    metrics::histogram!("comic_ocr_pipeline_seconds", "lang" => lang_label)
        .record(start.elapsed().as_secs_f64());
    result
}

fn run_blocking(
    detector: &Detector,
    recognizer: &dyn Recognizer,
    input: OcrInput,
    lang_label: &'static str,
) -> anyhow::Result<OcrOutput> {
    let (page_w, page_h) = (input.page_image.width(), input.page_image.height());
    // Clamp the user's region into page bounds defensively — the
    // handler already validated this, but we don't want a panic in
    // `crop_imm` if a future caller skipped validation.
    let clamped = Rect {
        x: input.region.x.min(page_w.saturating_sub(1)),
        y: input.region.y.min(page_h.saturating_sub(1)),
        w: input.region.w.min(page_w - input.region.x.min(page_w)),
        h: input.region.h.min(page_h - input.region.y.min(page_h)),
    };
    if clamped.w == 0 || clamped.h == 0 {
        return Err(anyhow::anyhow!("region collapsed to zero area"));
    }
    let margin = margin_for(&clamped);
    let crop = Rect {
        x: clamped.x.saturating_sub(margin),
        y: clamped.y.saturating_sub(margin),
        w: (clamped.w + 2 * margin).min(page_w - clamped.x.saturating_sub(margin)),
        h: (clamped.h + 2 * margin).min(page_h - clamped.y.saturating_sub(margin)),
    };
    let cropped = input.page_image.crop_imm(crop.x, crop.y, crop.w, crop.h);

    // User's drag rect translated into crop-local coords.
    let local_user = Rect {
        x: clamped.x - crop.x,
        y: clamped.y - crop.y,
        w: clamped.w,
        h: clamped.h,
    };

    // Detect bubbles in the crop.
    let det_out = {
        let mut det = detector.lock()?;
        det.inference(&cropped, DETECTOR_CONF, DETECTOR_NMS)?
    };

    let refined_local = pick_bbox(
        &det_out.bboxes,
        &local_user,
        cropped.width(),
        cropped.height(),
    );
    let (final_img, refined_page) = if let Some(r) = refined_local {
        let img = cropped.crop_imm(r.x, r.y, r.w, r.h);
        let r_page = Rect {
            x: r.x + crop.x,
            y: r.y + crop.y,
            w: r.w,
            h: r.h,
        };
        (img, Some(r_page))
    } else {
        // No detector hit — recognize what the user gave us.
        let img = cropped.crop_imm(local_user.x, local_user.y, local_user.w, local_user.h);
        (img, None)
    };

    let recognize_start = std::time::Instant::now();
    let recognition = recognizer.recognize(&final_img)?;
    metrics::histogram!("comic_ocr_recognize_seconds", "lang" => lang_label)
        .record(recognize_start.elapsed().as_secs_f64());
    Ok(OcrOutput {
        recognition,
        refined_bbox: refined_page,
    })
}

/// Pick the detector bbox with the largest intersection area
/// against the user's rect. Discards candidates with zero overlap.
fn pick_bbox(
    bboxes: &[comic_text_detector::ClassifiedBbox],
    user: &Rect,
    canvas_w: u32,
    canvas_h: u32,
) -> Option<Rect> {
    bboxes
        .iter()
        .filter_map(|b| {
            let r = to_rect(b, canvas_w, canvas_h)?;
            let overlap = r.intersection(user)?.area();
            Some((overlap, r))
        })
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, r)| r)
}

fn to_rect(b: &comic_text_detector::ClassifiedBbox, w: u32, h: u32) -> Option<Rect> {
    let xmin = b.xmin.max(0.0).round() as i64;
    let ymin = b.ymin.max(0.0).round() as i64;
    let xmax = b.xmax.max(0.0).round() as i64;
    let ymax = b.ymax.max(0.0).round() as i64;
    if xmax <= xmin || ymax <= ymin {
        return None;
    }
    let rx = (xmin as u32).min(w);
    let ry = (ymin as u32).min(h);
    let rxmax = (xmax as u32).min(w);
    let rymax = (ymax as u32).min(h);
    if rxmax <= rx || rymax <= ry {
        return None;
    }
    Some(Rect {
        x: rx,
        y: ry,
        w: rxmax - rx,
        h: rymax - ry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_of_disjoint_rects_is_none() {
        let a = Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        let b = Rect {
            x: 100,
            y: 100,
            w: 10,
            h: 10,
        };
        assert!(a.intersection(&b).is_none());
    }

    #[test]
    fn intersection_of_overlapping_rects() {
        let a = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 20,
        };
        let b = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        let i = a.intersection(&b).expect("rects overlap");
        assert_eq!(
            i,
            Rect {
                x: 10,
                y: 10,
                w: 10,
                h: 10
            }
        );
    }

    #[test]
    fn margin_clamps_to_min_for_tiny_regions() {
        let tiny = Rect {
            x: 0,
            y: 0,
            w: 12,
            h: 12,
        };
        assert_eq!(margin_for(&tiny), 8);
    }

    #[test]
    fn margin_clamps_to_max_for_huge_regions() {
        let huge = Rect {
            x: 0,
            y: 0,
            w: 4096,
            h: 4096,
        };
        assert_eq!(margin_for(&huge), 64);
    }

    #[test]
    fn pick_bbox_returns_largest_overlap() {
        let user = Rect {
            x: 100,
            y: 100,
            w: 100,
            h: 100,
        };
        let bboxes = vec![
            // Tiny overlap.
            comic_text_detector::ClassifiedBbox {
                xmin: 99.0,
                ymin: 99.0,
                xmax: 110.0,
                ymax: 110.0,
                confidence: 0.9,
                class: 0,
            },
            // Big overlap — should be chosen.
            comic_text_detector::ClassifiedBbox {
                xmin: 120.0,
                ymin: 110.0,
                xmax: 250.0,
                ymax: 230.0,
                confidence: 0.7,
                class: 0,
            },
            // Disjoint — discarded.
            comic_text_detector::ClassifiedBbox {
                xmin: 500.0,
                ymin: 500.0,
                xmax: 600.0,
                ymax: 600.0,
                confidence: 0.95,
                class: 0,
            },
        ];
        let chosen = pick_bbox(&bboxes, &user, 1000, 1000).expect("should pick a bbox");
        assert_eq!(chosen.x, 120);
        assert_eq!(chosen.y, 110);
    }

    #[test]
    fn pick_bbox_returns_none_for_no_overlap() {
        let user = Rect {
            x: 0,
            y: 0,
            w: 50,
            h: 50,
        };
        let bboxes = vec![comic_text_detector::ClassifiedBbox {
            xmin: 500.0,
            ymin: 500.0,
            xmax: 600.0,
            ymax: 600.0,
            confidence: 0.9,
            class: 0,
        }];
        assert!(pick_bbox(&bboxes, &user, 1000, 1000).is_none());
    }
}
