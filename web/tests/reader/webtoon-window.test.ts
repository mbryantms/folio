import { describe, expect, it } from "vitest";

import {
  WEBTOON_WINDOW_RADIUS,
  computeWebtoonWindow,
  nextPersistedProgressPage,
  placeholderAspectRatio,
} from "@/lib/reader/webtoon-window";

describe("computeWebtoonWindow", () => {
  it("centers on the current page with the default radius", () => {
    expect(computeWebtoonWindow(50, 200)).toEqual({
      start: 50 - WEBTOON_WINDOW_RADIUS,
      end: 50 + WEBTOON_WINDOW_RADIUS,
    });
  });

  it("clamps start at 0 near the top", () => {
    expect(computeWebtoonWindow(2, 200, 5)).toEqual({ start: 0, end: 7 });
  });

  it("clamps end at the last page near the bottom", () => {
    expect(computeWebtoonWindow(198, 200, 5)).toEqual({ start: 193, end: 199 });
  });

  it("clamps an out-of-range currentPage into the issue", () => {
    expect(computeWebtoonWindow(999, 10, 5)).toEqual({ start: 4, end: 9 });
    expect(computeWebtoonWindow(-5, 10, 5)).toEqual({ start: 0, end: 5 });
  });

  it("handles tiny issues", () => {
    expect(computeWebtoonWindow(0, 1, 5)).toEqual({ start: 0, end: 0 });
  });

  it("yields an empty range for an empty issue", () => {
    const w = computeWebtoonWindow(0, 0);
    expect(w.end).toBeLessThan(w.start);
  });
});

describe("placeholderAspectRatio", () => {
  it("uses server-known dims", () => {
    expect(placeholderAspectRatio({ image_width: 1988, image_height: 3056 })).toBe(
      "1988 / 3056",
    );
  });

  it("falls back to 2/3 when dims are missing or invalid", () => {
    expect(placeholderAspectRatio(undefined)).toBe("2 / 3");
    expect(placeholderAspectRatio({})).toBe("2 / 3");
    expect(placeholderAspectRatio({ image_width: 0, image_height: 0 })).toBe(
      "2 / 3",
    );
    expect(
      placeholderAspectRatio({ image_width: -1, image_height: 100 }),
    ).toBe("2 / 3");
    expect(
      placeholderAspectRatio({ image_width: 100, image_height: null }),
    ).toBe("2 / 3");
  });
});

describe("nextPersistedProgressPage (risk #5 monotonic guard)", () => {
  it("advances forward", () => {
    expect(nextPersistedProgressPage(10, 12)).toBe(12);
  });

  it("holds the high-water mark on a backward move", () => {
    expect(nextPersistedProgressPage(30, 25)).toBe(30);
  });

  it("is stable at equality", () => {
    expect(nextPersistedProgressPage(7, 7)).toBe(7);
  });
});
