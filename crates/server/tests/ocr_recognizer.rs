//! Engine tests against the real models.
//!
//! Everything that needs no model file is unit-tested next to the code
//! (`ocr::bubbles`, `ocr::lines`, `ocr::recognizer::ctc`,
//! `ocr::postprocess`). The tests here load the pinned models and are
//! `#[ignore]` because they:
//!
//!  - fetch model files from Hugging Face on first run (24 MB for the
//!    current Western engines, 37 MB for every registered line
//!    recogniser, ~440 MB for manga-ocr), OR
//!  - need the Tesseract toolchain and language data (the legacy
//!    Western recogniser), OR
//!  - run real inference.
//!
//! Run them with `cargo test -p server --test ocr_recognizer -- --ignored`
//! (add `--skip manga` to leave the 440 MB download out). The
//! non-ignored case only checks that the trait shapes compile, which CI
//! runs on every push.
//!
//! The page and the balloon crops are under `tests/fixtures/ocr/`; their
//! provenance is in the README there.

use std::time::{Duration, Instant};

use image::{DynamicImage, GrayImage, Luma};
use server::ocr::EngineSet;
use server::ocr::bubbles::{
    BubbleDetector, CLASS_BUBBLE, CLASS_TEXT_BUBBLE, CLASS_TEXT_FREE, is_region_class,
};
use server::ocr::cache::{CachedBbox, CachedDetection};
use server::ocr::manga_crop::{MangaCrop, manga_crop};
use server::ocr::models;
use server::ocr::pipeline::{PageDetector, Rect};
use server::ocr::postprocess;
use server::ocr::recognizer::{
    Language, Recognizer,
    ctc::{CtcRecognizer, LineOcr},
    manga::MangaOcr,
    western::WesternOcr,
};

/// Compile-time witness that the engines satisfy the pipeline's traits.
/// If a future refactor accidentally drops a trait bound the test crate
/// won't compile — caught by `cargo check` before any model has to load.
fn _trait_witness<'a>(
    w: &'a WesternOcr,
    m: &'a MangaOcr,
    l: &'a LineOcr,
    d: &'a BubbleDetector,
) -> ([&'a dyn Recognizer; 3], &'a dyn PageDetector) {
    ([w, m, l], d)
}

#[test]
fn trait_witness_compiles() {
    // Body intentionally empty — value is in `_trait_witness`'s
    // existence. Keeping the wrapper test prevents a clippy /
    // `unused` lint from gating builds in CI.
}

// ─── Fixtures ────────────────────────────────────────────────────

fn fixture(name: &str) -> DynamicImage {
    let path = format!("{}/tests/fixtures/ocr/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    // The server's own page decode (size limits, EXIF orientation).
    server::util::image_decode::decode_limited(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"))
}

const PAGE: &str = "strange-terrors-6-p028.jpg";

/// Centre of every text region of the fixture page, in percent of the
/// page (x, y): eleven balloons and two captions, in reading order.
const REGION_CENTRES: [(f32, f32); 13] = [
    (21.55, 8.1),
    (62.05, 8.5),
    (83.35, 11.35),
    (27.25, 36.8),
    (10.65, 41.35),
    (28.5, 42.95),
    (41.5, 42.95),
    (65.05, 40.3),
    (85.05, 38.5),
    (7.3, 69.2),
    (26.5, 68.65),
    (48.1, 72.9),
    (76.15, 72.45),
];

/// A balloon crop: its file, the index of its region in
/// [`REGION_CENTRES`], where the detector's box for it sits on the page
/// (x, y, w, h), and what it says.
struct Balloon {
    file: &'static str,
    region: usize,
    rect: (u32, u32, u32, u32),
    text: &'static str,
}

const BALLOONS: [Balloon; 3] = [
    Balloon {
        file: "balloon-06.png",
        region: 5,
        rect: (282, 726, 213, 166),
        text: "THAT VOICE --IT SOUNDS LIKE IT'S COMING FROM THE GRAVE!",
    },
    Balloon {
        file: "balloon-08.png",
        region: 7,
        rect: (687, 660, 292, 190),
        text: "BUT WHY SHOULD HE WANT TO KEEP US FROM OPENING THE MINE? \
               IT WOULD GIVE HIM HIS OLD JOB BACK AGAIN.",
    },
    Balloon {
        file: "balloon-10.png",
        region: 9,
        rect: (24, 1244, 158, 108),
        text: "JOHN! LOOK OUT!",
    },
];

fn area(b: &CachedBbox) -> f32 {
    (b.xmax - b.xmin) * (b.ymax - b.ymin)
}

/// The regions of a detection: what clients are offered (balloons and
/// free text), without the text boxes inside balloons.
fn regions(d: &CachedDetection) -> impl Iterator<Item = &CachedBbox> {
    d.bboxes.iter().filter(|b| is_region_class(b.class))
}

/// The region a tap at a percent point reads: the smallest one holding it.
fn region_at(d: &CachedDetection, (px, py): (f32, f32)) -> Option<&CachedBbox> {
    let (x, y) = (px / 100.0 * d.page_w as f32, py / 100.0 * d.page_h as f32);
    regions(d)
        .filter(|b| x >= b.xmin && x <= b.xmax && y >= b.ymin && y <= b.ymax)
        .min_by(|a, b| area(a).total_cmp(&area(b)))
}

/// A region's whole-pixel rectangle, rounded like the pipeline's crop.
fn rect_of(b: &CachedBbox) -> (u32, u32, u32, u32) {
    let (x0, y0) = (b.xmin.round() as u32, b.ymin.round() as u32);
    (
        x0,
        y0,
        b.xmax.round() as u32 - x0,
        b.ymax.round() as u32 - y0,
    )
}

/// Read a balloon crop the way the pipeline does: recognise, then the
/// cleanup of the current engine set.
fn read(ocr: &LineOcr, crop: &DynamicImage) -> postprocess::Cleaned {
    let rec = ocr.recognize(crop).expect("recognize");
    postprocess::clean_for(rec, Language::Western, EngineSet::Current)
}

// ─── Current engines ─────────────────────────────────────────────

#[tokio::test]
#[ignore = "loads the pinned page detector (11 MB, fetched on first run)"]
async fn the_detector_finds_every_balloon_of_the_fixture_page() {
    let page = fixture(PAGE);
    assert_eq!((page.width(), page.height()), (1280, 1876), "not square");
    let detector = BubbleDetector::shared().await.expect("detector");
    let found = detector.detect(&page).expect("detect");
    assert_eq!((found.page_w, found.page_h), (1280, 1876));

    // Every box is one of the model's three classes, confident, and on
    // the page.
    for b in &found.bboxes {
        assert!(
            [CLASS_BUBBLE, CLASS_TEXT_BUBBLE, CLASS_TEXT_FREE].contains(&b.class),
            "{b:?}"
        );
        assert!((0.5..=1.0).contains(&b.confidence), "{b:?}");
        assert!(b.xmin >= 0.0 && b.ymin >= 0.0, "{b:?}");
        assert!(b.xmax <= 1280.0 && b.ymax <= 1876.0, "{b:?}");
        assert!(b.xmax > b.xmin && b.ymax > b.ymin, "{b:?}");
    }
    // The page has thirteen regions (balloons and captions); the
    // detector does not flood it. The detection also keeps the text
    // boxes inside the balloons, which are not regions.
    let region_count = regions(&found).count();
    assert!((13..=16).contains(&region_count), "{region_count} regions");
    let text_boxes = found
        .bboxes
        .iter()
        .filter(|b| b.class == CLASS_TEXT_BUBBLE)
        .count();
    assert!(text_boxes >= 10, "{text_boxes} text boxes");

    // Each known region centre is under a box, and no two regions share
    // one (no balloons merged on this page).
    let mut picked: Vec<(u32, u32, u32, u32)> = Vec::new();
    for (i, centre) in REGION_CENTRES.iter().enumerate() {
        let b = region_at(&found, *centre)
            .unwrap_or_else(|| panic!("no region under the centre of region {}", i + 1));
        picked.push(rect_of(b));
    }
    let mut distinct = picked.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), REGION_CENTRES.len(), "{picked:?}");

    // The boxes of the three balloons the crops were cut from are where
    // the crops say (a pixel or two of slack for a runtime's arithmetic).
    for balloon in &BALLOONS {
        let got = picked[balloon.region];
        let want = balloon.rect;
        let near = |a: u32, b: u32| a.abs_diff(b) <= 2;
        assert!(
            near(got.0, want.0)
                && near(got.1, want.1)
                && near(got.2, want.2)
                && near(got.3, want.3),
            "{}: detector box {got:?}, crop cut at {want:?}",
            balloon.file
        );

        // Each of them holds exactly one clear text box, so the text-box
        // mode of the Japanese crop narrows the balloon to it (on real
        // detector output; which mode reads Japanese better is a
        // separate, unmeasured question).
        let region = Rect {
            x: got.0,
            y: got.1,
            w: got.2,
            h: got.3,
        };
        let crop = |mode| manga_crop(mode, region, &found.bboxes, found.page_w, found.page_h);
        assert_eq!(crop(MangaCrop::Region), region);
        let text = crop(MangaCrop::TextInRegion);
        assert_ne!(text, region, "{}: no text box found", balloon.file);
        assert!(
            text.w * text.h < region.w * region.h,
            "{}: text box {text:?} is not smaller than the balloon {region:?}",
            balloon.file
        );
        let (cx, cy) = (text.x + text.w / 2, text.y + text.h / 2);
        assert!(
            cx > region.x && cx < region.x + region.w && cy > region.y && cy < region.y + region.h,
            "{}: text box {text:?} is not in the balloon {region:?}",
            balloon.file
        );
    }
}

#[tokio::test]
#[ignore = "loads the pinned line finder and line recogniser (13 MB, fetched on first run)"]
async fn the_line_recogniser_reads_the_fixture_balloons() {
    let ocr = LineOcr::shared().await.expect("line recogniser");
    assert_eq!(ocr.recognizer().model().id, models::WESTERN_REC.id);
    for balloon in &BALLOONS {
        let crop = fixture(balloon.file);
        assert_eq!(
            (crop.width(), crop.height()),
            (balloon.rect.2, balloon.rect.3)
        );
        let out = read(ocr, &crop);
        assert_eq!(out.text, balloon.text, "{}", balloon.file);
        assert!(
            out.confidence > 0.9 && out.confidence <= 1.0,
            "{}: confidence {}",
            balloon.file,
            out.confidence
        );
        // One entry per line read, in reading order, inside the crop;
        // together they are the text (no line of these ends in a hyphen).
        let lines = out.words.expect("line entries");
        assert!(lines.len() >= 2, "{}: {} lines", balloon.file, lines.len());
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line.line_index, i as u32);
            assert!(!line.text.is_empty());
            assert!((0.0..=1.0).contains(&line.confidence));
            assert!(line.xmin >= 0.0 && line.ymin >= 0.0, "{line:?}");
            assert!(line.xmax > line.xmin && line.ymax > line.ymin, "{line:?}");
            assert!(
                line.xmax <= crop.width() as f32 && line.ymax <= crop.height() as f32,
                "{line:?}"
            );
        }
        let joined = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(joined, balloon.text, "{}", balloon.file);
    }
}

/// The reader's tap, end to end on the page: detect, take the region
/// under the point, crop, read.
#[tokio::test]
#[ignore = "loads the three pinned Western models (24 MB, fetched on first run)"]
async fn a_tap_on_the_page_reads_the_balloon_under_it() {
    let page = fixture(PAGE);
    let detector = BubbleDetector::shared().await.expect("detector");
    let ocr = LineOcr::shared().await.expect("line recogniser");
    let found = detector.detect(&page).expect("detect");
    for balloon in &BALLOONS {
        let region = region_at(&found, REGION_CENTRES[balloon.region]).expect("a region");
        let (x, y, w, h) = rect_of(region);
        let out = read(ocr, &page.crop_imm(x, y, w, h));
        assert_eq!(out.text, balloon.text, "{}", balloon.file);
    }
}

#[tokio::test]
#[ignore = "loads the pinned line finder and line recogniser (13 MB, fetched on first run)"]
async fn a_blank_crop_reads_as_nothing() {
    let ocr = LineOcr::shared().await.expect("line recogniser");
    for (w, h) in [(120, 60), (3, 3), (1, 400)] {
        let out = read(ocr, &blank_tile(w, h));
        assert_eq!(out.text, "", "{w}x{h}");
        assert!((0.0..=1.0).contains(&out.confidence));
        assert!(out.words.is_none(), "{w}x{h}");
    }
}

/// Every line recogniser in the registry loads: its pinned files verify,
/// its dictionary parses, and the dictionary fits the model's classes
/// (the check `CtcRecognizer::load` refuses to start without).
#[tokio::test]
#[ignore = "fetches every registered line recogniser on first run (37 MB)"]
async fn every_registered_line_recogniser_loads_with_a_matching_dictionary() {
    let expected = [
        ("en-pp-ocrv5-mobile-rec", 436),
        ("latin-pp-ocrv5-mobile-rec", 836),
        ("pp-ocrv6-small-rec", 18_708),
    ];
    assert_eq!(models::LINE_REC_MODELS.len(), expected.len());
    let crop = fixture(BALLOONS[2].file);
    for (model, (id, characters)) in models::LINE_REC_MODELS.iter().zip(expected) {
        assert_eq!(model.id, id);
        let rec = CtcRecognizer::load(model)
            .await
            .unwrap_or_else(|e| panic!("{id}: {e:#}"));
        assert_eq!(rec.dictionary_len(), characters, "{id}");
        // Each reads a three-line balloon given whole (one "line").
        let read = rec.read(std::slice::from_ref(&crop)).expect("read");
        assert_eq!(read.len(), 1, "{id}");
        assert!((0.0..=1.0).contains(&read[0].confidence), "{id}");
    }
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn median(sorted: &[f64]) -> f64 {
    sorted[sorted.len() / 2]
}

/// Prints how long the engines take on the fixture page: one detection,
/// then every region read the way a tap reads it. Not a benchmark: run
/// it in release, pinned to the cores to imitate
/// (`taskset -c 0,2,4,6 cargo test --release -p server --test
/// ocr_recognizer -- --ignored --nocapture timing`), on an idle machine.
/// The only assertion is that nothing is absurdly slow.
#[tokio::test]
#[ignore = "timing report; loads the three pinned Western models"]
#[expect(clippy::print_stderr, reason = "the report is the point")]
async fn timing_on_the_fixture_page() {
    const WARM_RUNS: usize = 5;
    let load_before = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let page = fixture(PAGE);
    let detector = BubbleDetector::shared().await.expect("detector");
    let ocr = LineOcr::shared().await.expect("line recogniser");

    let started = Instant::now();
    let found = detector.detect(&page).expect("detect");
    let first_detect = millis(started.elapsed());
    let mut detect: Vec<f64> = (0..WARM_RUNS)
        .map(|_| {
            let t = Instant::now();
            detector.detect(&page).expect("detect");
            millis(t.elapsed())
        })
        .collect();
    detect.sort_by(f64::total_cmp);

    let crops: Vec<DynamicImage> = regions(&found)
        .map(|b| {
            let (x, y, w, h) = rect_of(b);
            page.crop_imm(x, y, w, h)
        })
        .collect();
    let started = Instant::now();
    read(ocr, &crops[0]);
    let first_read = millis(started.elapsed());
    let mut reads: Vec<f64> = Vec::new();
    let mut pages: Vec<f64> = Vec::new();
    for _ in 0..WARM_RUNS {
        let page_started = Instant::now();
        for crop in &crops {
            let t = Instant::now();
            read(ocr, crop);
            reads.push(millis(t.elapsed()));
        }
        pages.push(millis(page_started.elapsed()));
    }
    reads.sort_by(f64::total_cmp);
    pages.sort_by(f64::total_cmp);
    let load_after = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();

    eprintln!(
        "profile: {}; cores available: {}; regions on the page: {}",
        if cfg!(debug_assertions) {
            "debug (not representative)"
        } else {
            "release"
        },
        std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
        crops.len()
    );
    eprintln!("load average before: {}", load_before.trim());
    eprintln!("load average after:  {}", load_after.trim());
    eprintln!(
        "detection:   first {first_detect:.0} ms; warm median {:.0} ms (min {:.0}, max {:.0}, n={})",
        median(&detect),
        detect[0],
        detect[detect.len() - 1],
        detect.len()
    );
    eprintln!(
        "one balloon: first {first_read:.0} ms; warm median {:.0} ms, p90 {:.0} ms (min {:.0}, max {:.0}, n={})",
        median(&reads),
        reads[reads.len() * 9 / 10],
        reads[0],
        reads[reads.len() - 1],
        reads.len()
    );
    eprintln!(
        "every balloon of the page: median {:.0} ms (n={})",
        median(&pages),
        pages.len()
    );
    assert!(median(&detect) < 10_000.0 && median(&reads) < 10_000.0);
}

// ─── Legacy Western recogniser and manga-ocr ─────────────────────

/// Blank white grayscale tile: every recogniser must answer it without
/// panicking, with a well-shaped result.
fn blank_tile(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([255])))
}

#[tokio::test]
#[ignore = "requires the build-time-compiled tesseract toolchain"]
async fn western_recognizes_a_blank_tile_without_panicking() {
    let ocr = WesternOcr::shared()
        .await
        .expect("western init should succeed once tessdata is staged");
    let out = ocr
        .recognize(&blank_tile(120, 60))
        .expect("recognize should return Ok even for empty input");
    // No text expected — blank image. We only assert the contract.
    assert!(
        out.text.is_empty(),
        "blank tile produced text: {:?}",
        out.text
    );
    assert!(
        (0.0..=1.0).contains(&out.confidence),
        "confidence outside [0, 1]: {}",
        out.confidence
    );
    assert!(
        out.words.is_none(),
        "a blank tile must not yield word boxes"
    );
}

#[tokio::test]
#[ignore = "requires the build-time-compiled tesseract toolchain"]
async fn western_word_boxes_stay_in_crop_bounds() {
    let ocr = WesternOcr::shared()
        .await
        .expect("western init should succeed once tessdata is staged");
    // Hand-synthesized glyphs are too brittle to assert recognized
    // content here; real-text word coverage lives in the golden
    // fixture suite (ocr_golden.rs). This pins the geometry
    // contract: word boxes, when present, are mapped back out of
    // the padded/upscaled space into crop coordinates.
    let out = ocr
        .recognize(&blank_tile(120, 60))
        .expect("recognize should return Ok");
    if let Some(words) = out.words {
        for w in &words {
            assert!((0.0..=1.0).contains(&w.confidence));
            assert!(w.xmax >= w.xmin && w.ymax >= w.ymin);
            assert!(w.xmax <= 120.0 && w.ymax <= 60.0);
        }
    }
}

#[tokio::test]
#[ignore = "requires the manga-ocr ONNX models (HF download, ~440 MB)"]
async fn manga_recognizes_a_blank_tile_without_panicking() {
    let ocr = MangaOcr::shared()
        .await
        .expect("manga-ocr init should succeed once models are cached");
    let out = ocr
        .recognize(&blank_tile(224, 224))
        .expect("recognize should return Ok even for empty input");
    // manga-ocr is greedy-decode and may hallucinate a short string
    // on blank input — we don't assert text content, only envelope.
    assert!(
        (0.0..=1.0).contains(&out.confidence),
        "confidence outside [0, 1]: {}",
        out.confidence
    );
    assert!(
        out.words.is_none(),
        "manga-ocr doesn't expose per-word boxes"
    );
}
