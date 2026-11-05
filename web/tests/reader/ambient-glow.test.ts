import { describe, expect, it } from "vitest";

import {
  combineSpreadEdges,
  edgeColorsFromPixels,
  glowBackground,
  tameGlowColor,
  unionBox,
  type EdgeColors,
  type Rgb,
} from "@/lib/reader/ambient-glow";

/** RGBA buffer where each pixel's colour comes from `at(x, y)`. */
function pixels(
  w: number,
  h: number,
  at: (x: number, y: number) => [number, number, number, number?],
): Uint8ClampedArray {
  const d = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const [r, g, b, a = 255] = at(x, y);
      d.set([r, g, b, a], (y * w + x) * 4);
    }
  }
  return d;
}

describe("edgeColorsFromPixels", () => {
  it("averages a strip along each edge", () => {
    // 8×8: red top rows, blue bottom rows, green elsewhere.
    const d = pixels(8, 8, (_x, y) =>
      y < 2 ? [255, 0, 0] : y >= 6 ? [0, 0, 255] : [0, 255, 0],
    );
    const e = edgeColorsFromPixels(d, 8, 8, 2);
    expect(e.top).toEqual([255, 0, 0]);
    expect(e.bottom).toEqual([0, 0, 255]);
    // Left/right strips span the whole height: 2 red + 4 green + 2 blue rows.
    expect(e.left).toEqual([64, 128, 64]);
    expect(e.right).toEqual(e.left);
  });

  it("separates left and right edges", () => {
    const d = pixels(16, 24, (x) =>
      x < 2 ? [10, 20, 30] : x >= 14 ? [200, 100, 50] : [0, 0, 0],
    );
    const e = edgeColorsFromPixels(d, 16, 24);
    expect(e.left).toEqual([10, 20, 30]);
    expect(e.right).toEqual([200, 100, 50]);
  });

  it("weights by alpha and treats a transparent strip as black", () => {
    const d = pixels(4, 4, (x) =>
      x === 0 ? [255, 255, 255, 0] : [100, 100, 100, 255],
    );
    const e = edgeColorsFromPixels(d, 4, 4, 1);
    expect(e.left).toEqual([0, 0, 0]);
    // Top row: one transparent white pixel + three opaque greys.
    expect(e.top).toEqual([100, 100, 100]);
  });
});

describe("combineSpreadEdges", () => {
  const a: EdgeColors = {
    top: [0, 0, 0],
    bottom: [10, 10, 10],
    left: [1, 1, 1],
    right: [2, 2, 2],
  };
  const b: EdgeColors = {
    top: [100, 100, 100],
    bottom: [30, 30, 30],
    left: [3, 3, 3],
    right: [4, 4, 4],
  };

  it("takes the outer sides and averages top / bottom", () => {
    expect(combineSpreadEdges([a, b])).toEqual({
      left: [1, 1, 1],
      right: [4, 4, 4],
      top: [50, 50, 50],
      bottom: [20, 20, 20],
    });
  });

  it("passes a single page through and returns null for none", () => {
    expect(combineSpreadEdges([a])).toBe(a);
    expect(combineSpreadEdges([])).toBeNull();
  });
});

describe("tameGlowColor", () => {
  it("scales bright colours down, keeping hue", () => {
    expect(tameGlowColor([255, 255, 255])).toEqual([170, 170, 170]);
    expect(tameGlowColor([255, 0, 128])).toEqual([170, 0, 85]);
  });
  it("leaves darker colours alone", () => {
    const c: Rgb = [120, 60, 30];
    expect(tameGlowColor(c)).toBe(c);
  });
});

describe("glowBackground", () => {
  it("anchors one gradient on each page edge", () => {
    const e: EdgeColors = {
      top: [10, 0, 0],
      bottom: [0, 10, 0],
      left: [0, 0, 10],
      right: [10, 10, 10],
    };
    const bg = glowBackground(e, {
      left: 100,
      top: 0,
      width: 400,
      height: 600,
    });
    expect(bg.match(/radial-gradient/g)).toHaveLength(4);
    expect(bg).toContain("at 100px 300px, rgb(0 0 10)"); // left edge
    expect(bg).toContain("at 500px 300px, rgb(10 10 10)"); // right edge
    expect(bg).toContain("at 300px 0px, rgb(10 0 0)"); // top edge
    expect(bg).toContain("at 300px 600px, rgb(0 10 0)"); // bottom edge
  });
});

describe("unionBox", () => {
  it("spans a spread and ignores empty boxes", () => {
    expect(
      unionBox([
        { left: 10, top: 5, width: 100, height: 200 },
        { left: 110, top: 0, width: 100, height: 210 },
        { left: 0, top: 0, width: 0, height: 0 },
      ]),
    ).toEqual({ left: 10, top: 0, width: 200, height: 210 });
    expect(unionBox([])).toBeNull();
  });
});
