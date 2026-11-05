import { describe, expect, it } from "vitest";

import { paletteFromPixels } from "@/lib/ui/cover-palette";

function pixels(...rgba: number[][]): Uint8ClampedArray {
  return new Uint8ClampedArray(rgba.flat());
}

describe("paletteFromPixels", () => {
  it("averages opaque pixels and picks the strongest mid-tone colour", () => {
    const p = paletteFromPixels(
      pixels([0, 0, 0, 255], [250, 250, 250, 255], [220, 40, 40, 255]),
    );
    expect(p?.average).toEqual([157, 97, 97]);
    expect(p?.vibrant).toEqual([220, 40, 40]);
  });

  it("falls back to the average for a grey cover", () => {
    const p = paletteFromPixels(
      pixels([90, 90, 90, 255], [110, 110, 110, 255]),
    );
    expect(p?.vibrant).toEqual(p?.average);
  });

  it("ignores transparent pixels and returns null when nothing is opaque", () => {
    expect(paletteFromPixels(pixels([255, 0, 0, 0]))).toBeNull();
  });
});
