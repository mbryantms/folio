//! Contact sheets: the page with every detected box drawn and numbered,
//! plus the reference's approximate boxes, so detection can be judged
//! by eye. Drawn by hand (a 3x5 digit font) to need no font file.

use std::path::Path;

use image::{DynamicImage, Rgb, RgbImage};

use crate::engines::DetBox;

pub(crate) const CLASS0: Rgb<u8> = Rgb([220, 20, 20]);
pub(crate) const CLASS1: Rgb<u8> = Rgb([20, 70, 230]);
pub(crate) const REFERENCE: Rgb<u8> = Rgb([0, 170, 60]);

/// 3x5 bitmaps for 0-9, rows top to bottom, 3 bits each (MSB = left).
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 2, 2, 2],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn fill(img: &mut RgbImage, x0: i64, y0: i64, x1: i64, y1: i64, c: Rgb<u8>) {
    let (w, h) = (i64::from(img.width()), i64::from(img.height()));
    for y in y0.max(0)..y1.min(h) {
        for x in x0.max(0)..x1.min(w) {
            img.put_pixel(x as u32, y as u32, c);
        }
    }
}

fn hollow(img: &mut RgbImage, b: [f64; 4], t: i64, c: Rgb<u8>) {
    let [x0, y0, x1, y1] = b.map(|v| v.round() as i64);
    fill(img, x0, y0, x1, y0 + t, c);
    fill(img, x0, y1 - t, x1, y1, c);
    fill(img, x0, y0, x0 + t, y1, c);
    fill(img, x1 - t, y0, x1, y1, c);
}

/// Number `n` at (x, y) in colour `c` on a white tag; `s` = pixel size.
fn label(img: &mut RgbImage, x: i64, y: i64, n: usize, s: i64, c: Rgb<u8>) {
    let digits: Vec<usize> = n
        .to_string()
        .bytes()
        .map(|b| usize::from(b - b'0'))
        .collect();
    let w = digits.len() as i64 * 4 * s + s;
    fill(img, x, y, x + w, y + 7 * s, Rgb([255, 255, 255]));
    for (i, d) in digits.iter().enumerate() {
        for (row, bits) in DIGITS[*d].iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    let px = x + s + i as i64 * 4 * s + col * s;
                    let py = y + s + row as i64 * s;
                    fill(img, px, py, px + s, py + s, c);
                }
            }
        }
    }
}

/// Write the contact sheet at half size as JPEG. Detected box `i` is
/// labelled `i + 1` (the order in the result JSON); reference boxes are
/// labelled with their position in the truth file.
pub(crate) fn contact_sheet(
    page: &DynamicImage,
    boxes: &[DetBox],
    refs: &[[f64; 4]],
    out: &Path,
) -> anyhow::Result<()> {
    let scale = 0.5;
    let small = page.resize(
        (f64::from(page.width()) * scale) as u32,
        (f64::from(page.height()) * scale) as u32,
        image::imageops::FilterType::Triangle,
    );
    let mut img = small.to_rgb8();
    for (i, r) in refs.iter().enumerate() {
        let b = r.map(|v| v * scale);
        hollow(&mut img, b, 2, REFERENCE);
        label(
            &mut img,
            b[2].round() as i64 - 30,
            b[3].round() as i64 - 4,
            i + 1,
            3,
            REFERENCE,
        );
    }
    for (i, d) in boxes.iter().enumerate() {
        let c = if d.class == 0 { CLASS0 } else { CLASS1 };
        let b = [d.x0, d.y0, d.x1, d.y1].map(|v| f64::from(v) * scale);
        hollow(&mut img, b, if d.class == 0 { 4 } else { 2 }, c);
        let (lx, ly) = if d.class == 0 {
            (b[0] as i64, b[1] as i64 - 22)
        } else {
            (b[0] as i64 + 4, b[1] as i64 + 4)
        };
        label(&mut img, lx, ly, i + 1, 3, c);
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    DynamicImage::ImageRgb8(img).save(out)?;
    Ok(())
}
