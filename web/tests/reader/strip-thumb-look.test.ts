import { describe, expect, it } from "vitest";

import { stripThumbLook } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageStrip";

const SCALES = [1.6, 1.32, 1.15, 1.06];

describe("stripThumbLook", () => {
  it("swells toward the open page and settles to 1 further out", () => {
    const sizes = [0, 1, 2, 3, 4, 9].map(
      (d) => stripThumbLook(d, SCALES, 100).scale,
    );
    expect(sizes).toEqual([1.6, 1.32, 1.15, 1.06, 1, 1]);
  });

  it("fades further pages back, never below the resting opacity", () => {
    const op = [0, 1, 2, 3, 20].map(
      (d) => stripThumbLook(d, SCALES, 100).opacity,
    );
    expect(op).toEqual([1, 0.9, 0.8, 0.7, 0.7]);
  });

  it("reports the overhang the slot margins must take up", () => {
    // A 100px thumb at 1.6x reaches 30px past each side.
    expect(stripThumbLook(0, SCALES, 100).overhang).toBeCloseTo(30);
    expect(stripThumbLook(5, SCALES, 100).overhang).toBe(0);
  });
});
