//! Splitting a balloon crop into text lines for the one-line PP-OCR
//! recognisers. Every splitter returns line rectangles in crop pixels,
//! in reading order; the lines are then cut from the colour crop.

use image::{DynamicImage, GrayImage, Luma};
use serde::Serialize;

use crate::cand::{DetResize, PaddleDet, reading_order};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum Split {
    /// PP-OCRv5 mobile det on the crop (size kept, rounded to 32); the
    /// whole crop as one line when it finds none.
    PpDet,
    /// The same on the crop enlarged 2x (long side capped at 1600).
    PpDet2x,
    /// Lines from PP-OCRv5 det run once on the full page (long side 960)
    /// that lie at least half inside the crop; whole crop when none.
    PageLines,
    /// Row projection profile on a binarised copy.
    Profile,
    /// comic-text-detector's class-1 (line) boxes inside the crop; the
    /// whole crop as one line when there are none.
    CtdLines,
    /// No split: the whole crop as one line (control).
    None,
}

impl Split {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Split::PpDet => "ppdet",
            Split::PpDet2x => "ppdet2x",
            Split::PageLines => "page-lines",
            Split::Profile => "profile",
            Split::CtdLines => "ctd-lines",
            Split::None => "none",
        }
    }
    pub(crate) const ALL: [Split; 6] = [
        Split::PpDet,
        Split::PpDet2x,
        Split::PageLines,
        Split::Profile,
        Split::CtdLines,
        Split::None,
    ];
}

pub(crate) type Rect = (u32, u32, u32, u32);

fn whole(crop: &DynamicImage) -> Vec<Rect> {
    vec![(0, 0, crop.width(), crop.height())]
}

/// Lines from the PP det on the crop (scaled by `scale` first when
/// given), in reading order; the whole crop when it finds none. Returns
/// whether it fell back.
pub(crate) fn ppdet_lines(
    det: &mut PaddleDet,
    crop: &DynamicImage,
    scale: Option<f64>,
) -> anyhow::Result<(Vec<Rect>, bool)> {
    let resize = scale.map_or(DetResize::Keep, DetResize::Scale);
    let mut boxes: Vec<[f32; 4]> = det
        .detect_lines(crop, resize)?
        .into_iter()
        .map(|b| [b.x0, b.y0, b.x1, b.y1])
        .collect();
    reading_order(&mut boxes);
    let rects = to_rects(&boxes, crop);
    if rects.is_empty() {
        return Ok((whole(crop), true));
    }
    Ok((rects, false))
}

/// comic-text-detector line boxes (already in crop coordinates) at least
/// half inside the crop, in reading order; the whole crop when none.
/// Returns whether it fell back.
pub(crate) fn ctd_lines(lines: &[[f32; 4]], crop: &DynamicImage) -> (Vec<Rect>, bool) {
    let (w, h) = (crop.width() as f32, crop.height() as f32);
    let mut inside: Vec<[f32; 4]> = lines
        .iter()
        .filter(|b| {
            let a = (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
            let iw = b[2].min(w) - b[0].max(0.0);
            let ih = b[3].min(h) - b[1].max(0.0);
            a > 0.0 && iw > 0.0 && ih > 0.0 && iw * ih / a >= 0.5
        })
        .copied()
        .collect();
    if inside.is_empty() {
        return (whole(crop), true);
    }
    reading_order(&mut inside);
    (to_rects(&inside, crop), false)
}

fn to_rects(boxes: &[[f32; 4]], crop: &DynamicImage) -> Vec<Rect> {
    let (w, h) = (crop.width(), crop.height());
    boxes
        .iter()
        .filter_map(|b| {
            let x0 = (b[0].max(0.0).round() as u32).min(w);
            let y0 = (b[1].max(0.0).round() as u32).min(h);
            let x1 = (b[2].max(0.0).round() as u32).min(w);
            let y1 = (b[3].max(0.0).round() as u32).min(h);
            (x1 > x0 + 2 && y1 > y0 + 2).then_some((x0, y0, x1 - x0, y1 - y0))
        })
        .collect()
}

/// Model-free split. Binarise a grey copy (Otsu; flip polarity when the
/// border ring is mostly dark), drop ink components that touch the crop
/// edge (balloon outline, art) or are huge, take the row profile, cut
/// bands of rows with ink, merge bands separated by a small gap, drop
/// slivers, and give each band its column extent plus a margin.
pub(crate) fn profile_lines(crop: &DynamicImage) -> Vec<Rect> {
    let gray = crop.to_luma8();
    let (w, h) = gray.dimensions();
    if w < 8 || h < 8 {
        return whole(crop);
    }
    let level = imageproc::contrast::otsu_level(&gray);
    let mut ink = GrayImage::new(w, h);
    let dark = |v: u8| v <= level;
    // Polarity from the border ring.
    let (mut ring_dark, mut ring) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            if y < 2 || y >= h - 2 || x < 2 || x >= w - 2 {
                ring += 1;
                if dark(gray.get_pixel(x, y).0[0]) {
                    ring_dark += 1;
                }
            }
        }
    }
    let inverted = ring_dark * 2 > ring;
    for (x, y, p) in gray.enumerate_pixels() {
        if dark(p.0[0]) != inverted {
            ink.put_pixel(x, y, Luma([255]));
        }
    }
    let labels = imageproc::region_labelling::connected_components(
        &ink,
        imageproc::region_labelling::Connectivity::Eight,
        Luma([0u8]),
    );
    let mut drop = std::collections::HashSet::new();
    let mut edge = std::collections::HashSet::new();
    let mut extent: std::collections::HashMap<u32, [u32; 4]> = std::collections::HashMap::new();
    let mut sizes: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for (x, y, l) in labels.enumerate_pixels() {
        let l = l.0[0];
        if l == 0 {
            continue;
        }
        *sizes.entry(l).or_default() += 1;
        if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
            edge.insert(l);
        }
        let e = extent.entry(l).or_insert([x, y, x, y]);
        e[0] = e[0].min(x);
        e[1] = e[1].min(y);
        e[2] = e[2].max(x);
        e[3] = e[3].max(y);
    }
    // An edge-touching component is dropped only when it is long (a
    // balloon outline or panel border); a letter cut by a rough crop
    // stays.
    for l in &edge {
        let e = extent[l];
        if e[2] - e[0] > w * 3 / 10 || e[3] - e[1] > h * 3 / 10 {
            drop.insert(*l);
        }
    }
    let big = u64::from(w) * u64::from(h) / 20;
    for (l, s) in &sizes {
        if *s > big {
            drop.insert(*l);
        }
    }
    let keep = |x: u32, y: u32| {
        let l = labels.get_pixel(x, y).0[0];
        l != 0 && !drop.contains(&l)
    };
    let rows: Vec<u32> = (0..h)
        .map(|y| (0..w).filter(|&x| keep(x, y)).count() as u32)
        .collect();
    let min_ink = (w / 200).max(1);
    let mut bands: Vec<(u32, u32)> = Vec::new();
    let mut start = None;
    for (y, &c) in rows.iter().enumerate() {
        let y = y as u32;
        match (c >= min_ink, start) {
            (true, None) => start = Some(y),
            (false, Some(s)) => {
                bands.push((s, y));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        bands.push((s, h));
    }
    if bands.is_empty() {
        return whole(crop);
    }
    let mut heights: Vec<u32> = bands.iter().map(|(a, b)| b - a).collect();
    heights.sort_unstable();
    let med = f64::from(heights[heights.len() / 2]).max(1.0);
    // Merge across gaps smaller than a fifth of a line (dots, accents).
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for b in bands {
        if let Some(last) = merged.last_mut()
            && f64::from(b.0 - last.1) < 0.2 * med
        {
            last.1 = b.1;
            continue;
        }
        merged.push(b);
    }
    let mut out = Vec::new();
    for (y0, y1) in merged {
        if f64::from(y1 - y0) < 0.35 * med {
            continue;
        }
        let (mut x0, mut x1) = (w, 0);
        for y in y0..y1 {
            for x in 0..w {
                if keep(x, y) {
                    x0 = x0.min(x);
                    x1 = x1.max(x + 1);
                }
            }
        }
        if x1 <= x0 {
            continue;
        }
        let m = ((f64::from(y1 - y0) * 0.2) as u32).max(2);
        let (ax, ay) = (x0.saturating_sub(m), y0.saturating_sub(m));
        let (bx, by) = ((x1 + m).min(w), (y1 + m).min(h));
        out.push((ax, ay, bx - ax, by - ay));
    }
    if out.is_empty() { whole(crop) } else { out }
}

/// Cut the line images from the colour crop.
pub(crate) fn cut(crop: &DynamicImage, rects: &[Rect]) -> Vec<DynamicImage> {
    rects
        .iter()
        .filter(|r| r.2 >= 4 && r.3 >= 4)
        .map(|r| crop.crop_imm(r.0, r.1, r.2, r.3))
        .collect()
}

pub(crate) fn none_lines(crop: &DynamicImage) -> Vec<Rect> {
    whole(crop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_finds_two_lines_and_ignores_the_outline() {
        let mut g = GrayImage::from_pixel(200, 100, Luma([255]));
        // Outline touching the edge.
        for x in 0..200 {
            g.put_pixel(x, 0, Luma([0]));
            g.put_pixel(x, 99, Luma([0]));
        }
        for (y0, y1) in [(20, 35), (55, 70)] {
            for y in y0..y1 {
                for x in (30..170).step_by(3) {
                    g.put_pixel(x, y, Luma([10]));
                }
            }
        }
        let lines = profile_lines(&DynamicImage::ImageLuma8(g));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].1 < 20 && lines[0].1 + lines[0].3 > 35);
        assert!(lines[1].1 < 55 && lines[1].1 + lines[1].3 > 70);
    }
}
