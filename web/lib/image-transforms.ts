/**
 * Client-side mirrors of the server's page-image transforms
 * (`archive-rewrite-1.0` M5). The page editor runs these over a page's
 * strip-thumb `ImageData` to show a live preview of what the server will
 * write on save; the Rust counterparts live in
 * `crates/server/src/jobs/archive_transforms.rs`.
 *
 * The intent is algorithmic parity, not byte-exact parity — the server
 * decodes the full-resolution page while the preview runs on a small thumb,
 * and the gaussian/median kernels are reimplemented here in JS. Each
 * function is pure (returns a fresh buffer) and framework-free so it can be
 * unit-tested without a DOM. Inputs are clamped to the same ranges the Rust
 * side enforces, so a preview never diverges in validity from the apply.
 */
import type { TransformStep } from "@/lib/api/types";

/** A mutable RGBA raster. Structurally compatible with the DOM `ImageData`,
 *  so callers can pass a canvas `ImageData` straight in. */
export interface RgbaImage {
  data: Uint8ClampedArray;
  width: number;
  height: number;
}

function clamp8(v: number): number {
  return v < 0 ? 0 : v > 255 ? 255 : Math.round(v);
}

function clone(img: RgbaImage): RgbaImage {
  return {
    data: new Uint8ClampedArray(img.data),
    width: img.width,
    height: img.height,
  };
}

/** Apply a 256-entry LUT to the R/G/B channels, leaving alpha untouched. */
function mapRgb(img: RgbaImage, lut: Uint8ClampedArray): RgbaImage {
  const out = new Uint8ClampedArray(img.data.length);
  for (let i = 0; i < img.data.length; i += 4) {
    out[i] = lut[img.data[i]];
    out[i + 1] = lut[img.data[i + 1]];
    out[i + 2] = lut[img.data[i + 2]];
    out[i + 3] = img.data[i + 3];
  }
  return { data: out, width: img.width, height: img.height };
}

/** Additive brightness + multiplicative contrast about mid-grey. Mirrors
 *  `brightness_contrast` in archive_transforms.rs (same GIMP-style curve). */
export function brightnessContrast(
  img: RgbaImage,
  brightness: number,
  contrast: number,
): RgbaImage {
  const b = Math.max(-100, Math.min(100, brightness));
  const c = Math.max(-100, Math.min(100, contrast));
  if (b === 0 && c === 0) return clone(img);
  const factor = (259 * (c + 255)) / (255 * (259 - c));
  const lut = new Uint8ClampedArray(256);
  for (let i = 0; i < 256; i++) lut[i] = clamp8(factor * (i - 128) + 128 + b);
  return mapRgb(img, lut);
}

/** Stretch `[lo, hi]` to `[0, 255]` per channel. No-op when `lo >= hi`. */
export function levelsClip(img: RgbaImage, lo: number, hi: number): RgbaImage {
  if (lo >= hi) return clone(img);
  const span = hi - lo;
  const lut = new Uint8ClampedArray(256);
  for (let i = 0; i < 256; i++) lut[i] = clamp8(((i - lo) / span) * 255);
  return mapRgb(img, lut);
}

function gaussianKernel(sigma: number): number[] {
  const radius = Math.max(1, Math.ceil(sigma * 3));
  const k: number[] = [];
  let sum = 0;
  for (let i = -radius; i <= radius; i++) {
    const v = Math.exp(-(i * i) / (2 * sigma * sigma));
    k.push(v);
    sum += v;
  }
  return k.map((v) => v / sum);
}

/** Separable gaussian blur with clamp-to-edge borders. */
function gaussianBlur(img: RgbaImage, sigma: number): RgbaImage {
  const kernel = gaussianKernel(sigma);
  const radius = (kernel.length - 1) / 2;
  const { width: w, height: h, data } = img;
  const tmp = new Float32Array(data.length);
  // Horizontal pass.
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      for (let ch = 0; ch < 3; ch++) {
        let acc = 0;
        for (let k = -radius; k <= radius; k++) {
          const xx = Math.min(w - 1, Math.max(0, x + k));
          acc += data[(y * w + xx) * 4 + ch] * kernel[k + radius];
        }
        tmp[(y * w + x) * 4 + ch] = acc;
      }
    }
  }
  // Vertical pass.
  const out = new Uint8ClampedArray(data.length);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      for (let ch = 0; ch < 3; ch++) {
        let acc = 0;
        for (let k = -radius; k <= radius; k++) {
          const yy = Math.min(h - 1, Math.max(0, y + k));
          acc += tmp[(yy * w + x) * 4 + ch] * kernel[k + radius];
        }
        out[(y * w + x) * 4 + ch] = clamp8(acc);
      }
      out[(y * w + x) * 4 + 3] = data[(y * w + x) * 4 + 3];
    }
  }
  return { data: out, width: w, height: h };
}

/** Unsharp mask: `out = orig + (orig - blurred)`, matching the `image`
 *  crate's `unsharpen` (amount fixed, threshold 0). `amount` is the
 *  gaussian sigma, clamped to `0..=5`; `<= 0` is a no-op. */
export function sharpen(img: RgbaImage, amount: number): RgbaImage {
  const sigma = Math.max(0, Math.min(5, amount));
  if (sigma <= 0) return clone(img);
  const blurred = gaussianBlur(img, sigma);
  const out = new Uint8ClampedArray(img.data.length);
  for (let i = 0; i < img.data.length; i += 4) {
    for (let ch = 0; ch < 3; ch++) {
      const o = img.data[i + ch];
      out[i + ch] = clamp8(o + (o - blurred.data[i + ch]));
    }
    out[i + 3] = img.data[i + 3];
  }
  return { data: out, width: img.width, height: img.height };
}

/** Median filter over a square window of `radius` (clamped `1..=4`),
 *  clamp-to-edge borders. Erases lone specks. */
export function despeckle(img: RgbaImage, radius: number): RgbaImage {
  const r = Math.max(1, Math.min(4, Math.round(radius)));
  const { width: w, height: h, data } = img;
  const out = new Uint8ClampedArray(data.length);
  const win: number[] = [];
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      for (let ch = 0; ch < 3; ch++) {
        win.length = 0;
        for (let dy = -r; dy <= r; dy++) {
          for (let dx = -r; dx <= r; dx++) {
            const xx = Math.min(w - 1, Math.max(0, x + dx));
            const yy = Math.min(h - 1, Math.max(0, y + dy));
            win.push(data[(yy * w + xx) * 4 + ch]);
          }
        }
        win.sort((a, b) => a - b);
        out[(y * w + x) * 4 + ch] = win[(win.length - 1) >> 1];
      }
      out[(y * w + x) * 4 + 3] = data[(y * w + x) * 4 + 3];
    }
  }
  return { data: out, width: w, height: h };
}

/** Crop to `(x, y, w, h)`, clamped to the image; zero-area → no-op. */
export function crop(
  img: RgbaImage,
  x: number,
  y: number,
  w: number,
  h: number,
): RgbaImage {
  const ix = Math.min(Math.max(0, Math.round(x)), img.width);
  const iy = Math.min(Math.max(0, Math.round(y)), img.height);
  const cw = Math.min(Math.round(w), img.width - ix);
  const ch = Math.min(Math.round(h), img.height - iy);
  if (cw <= 0 || ch <= 0) return clone(img);
  const out = new Uint8ClampedArray(cw * ch * 4);
  for (let row = 0; row < ch; row++) {
    const srcStart = ((iy + row) * img.width + ix) * 4;
    out.set(img.data.subarray(srcStart, srcStart + cw * 4), row * cw * 4);
  }
  return { data: out, width: cw, height: ch };
}

/** Fold a transform chain over `img`, in order. Crop changes dimensions. */
export function applyChain(img: RgbaImage, chain: TransformStep[]): RgbaImage {
  return chain.reduce<RgbaImage>((acc, step) => {
    switch (step.kind) {
      case "brightness_contrast":
        return brightnessContrast(acc, step.brightness, step.contrast);
      case "levels_clip":
        return levelsClip(acc, step.lo, step.hi);
      case "sharpen":
        return sharpen(acc, step.amount);
      case "despeckle":
        return despeckle(acc, step.radius);
      case "crop_box":
        return crop(acc, step.x, step.y, step.w, step.h);
      default:
        return acc;
    }
  }, img);
}
