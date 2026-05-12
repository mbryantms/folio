/**
 * M4 — `readingPercent` is the value the top progress bar binds its width
 * to. Trivial pure helper, but worth pinning the boundary cases (zero,
 * single-page, last page) so future tweaks don't subtly shift the bar.
 */
import { describe, expect, it } from "vitest";
import { readingPercent } from "@/lib/reader/fullscreen";

describe("readingPercent", () => {
  it("returns 0 when there are no pages", () => {
    expect(readingPercent(0, 0)).toBe(0);
    expect(readingPercent(5, 0)).toBe(0);
  });

  it("returns 100 on the only page of a single-page issue", () => {
    expect(readingPercent(0, 1)).toBe(100);
  });

  it("scales linearly through the issue", () => {
    expect(readingPercent(0, 10)).toBe(10);
    expect(readingPercent(4, 10)).toBe(50);
    expect(readingPercent(9, 10)).toBe(100);
  });

  it("clamps negative pages to 0", () => {
    expect(readingPercent(-1, 10)).toBe(0);
    expect(readingPercent(-50, 10)).toBe(0);
  });

  it("clamps overshoot to 100", () => {
    expect(readingPercent(20, 10)).toBe(100);
  });
});
