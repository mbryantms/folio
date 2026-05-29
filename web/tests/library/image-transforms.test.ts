import { describe, expect, it } from "vitest";

import {
  applyChain,
  brightnessContrast,
  crop,
  despeckle,
  levelsClip,
  sharpen,
  type RgbaImage,
} from "@/lib/image-transforms";
import type { TransformStep } from "@/lib/api/types";

/** Build a solid RGBA raster. */
function solid(width: number, height: number, v: number): RgbaImage {
  const data = new Uint8ClampedArray(width * height * 4);
  for (let i = 0; i < data.length; i += 4) {
    data[i] = v;
    data[i + 1] = v;
    data[i + 2] = v;
    data[i + 3] = 255;
  }
  return { data, width, height };
}

/** Build a 1-row raster from explicit grey values. */
function row(values: number[]): RgbaImage {
  const data = new Uint8ClampedArray(values.length * 4);
  values.forEach((v, i) => {
    data[i * 4] = v;
    data[i * 4 + 1] = v;
    data[i * 4 + 2] = v;
    data[i * 4 + 3] = 255;
  });
  return { data, width: values.length, height: 1 };
}

describe("image-transforms", () => {
  it("brightness raises and lowers the mean; zero is identity", () => {
    const base = solid(4, 4, 100);
    expect(brightnessContrast(base, 50, 0).data[0]).toBe(150);
    expect(brightnessContrast(base, -40, 0).data[0]).toBe(60);
    const noop = brightnessContrast(base, 0, 0);
    expect(Array.from(noop.data)).toEqual(Array.from(base.data));
  });

  it("levelsClip maps the endpoints to 0 and 255", () => {
    const out = levelsClip(row([64, 128, 192]), 64, 192);
    expect(out.data[0]).toBe(0); // 64 → 0
    expect(out.data[8]).toBe(255); // 192 → 255
    // No-op when lo >= hi.
    const same = levelsClip(row([10, 20]), 200, 100);
    expect(Array.from(same.data)).toEqual([10, 10, 10, 255, 20, 20, 20, 255]);
  });

  it("sharpen keeps dims and is a no-op at zero", () => {
    const base = solid(8, 8, 120);
    const s = sharpen(base, 2);
    expect([s.width, s.height]).toEqual([8, 8]);
    const noop = sharpen(base, 0);
    expect(Array.from(noop.data)).toEqual(Array.from(base.data));
  });

  it("despeckle erases a lone speck", () => {
    const img = solid(8, 8, 10);
    // inject a bright speck at (4,4)
    const idx = (4 * 8 + 4) * 4;
    img.data[idx] = 250;
    img.data[idx + 1] = 250;
    img.data[idx + 2] = 250;
    const out = despeckle(img, 1);
    expect(out.data[idx]).toBe(10);
  });

  it("crop yields exact dims and clamps an oversized box", () => {
    const base = solid(20, 20, 50);
    expect([crop(base, 2, 2, 6, 8).width, crop(base, 2, 2, 6, 8).height]).toEqual(
      [6, 8],
    );
    const clamped = crop(base, 8, 8, 999, 999);
    expect([clamped.width, clamped.height]).toEqual([12, 12]);
    // Fully out of bounds → unchanged.
    const oob = crop(base, 99, 99, 4, 4);
    expect([oob.width, oob.height]).toEqual([20, 20]);
  });

  it("applyChain folds steps in order and is deterministic", () => {
    const chain: TransformStep[] = [
      { kind: "brightness_contrast", brightness: 20, contrast: 10 },
      { kind: "levels_clip", lo: 10, hi: 240 },
      { kind: "crop_box", x: 1, y: 1, w: 10, h: 10 },
    ];
    const a = applyChain(solid(16, 16, 100), chain);
    const b = applyChain(solid(16, 16, 100), chain);
    expect([a.width, a.height]).toEqual([10, 10]);
    expect(Array.from(a.data)).toEqual(Array.from(b.data));
  });
});
