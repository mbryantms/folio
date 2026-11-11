// @vitest-environment jsdom
/**
 * The app-wide cover size (lib/library/cover-size.ts): the pre-paint
 * script mirrors the stored size onto <html>, the slider's stops are the
 * column counts that fill the library row, and the target stored for a
 * stop reproduces that count.
 */
import { beforeEach, describe, expect, it } from "vitest";

import {
  COVER_GRID_COLUMNS,
  COVER_SIZE_BOOTSTRAP,
  COVER_SIZE_KEY,
  COVER_STOP_MAX_PX,
  COVER_STOP_MIN_PX,
  COVER_WIDTH,
  COVER_WIDTH_KEY,
  LEGACY_LIBRARY_SIZE_KEY,
  coverColumnWidth,
  coverStops,
  coverTargetForColumns,
  parseCoverSize,
  syncCoverSizeVars,
} from "@/lib/library/cover-size";
import {
  computeColumnWidth,
  computeColumnsPerRow,
} from "@/lib/library/grid-window";

const v = (name: string) =>
  document.documentElement.style.getPropertyValue(name);

describe("cover-size variables", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute("style");
  });

  it("mirrors the stored size and width", () => {
    localStorage.setItem(COVER_SIZE_KEY, "128");
    localStorage.setItem(COVER_WIDTH_KEY, "136.25");
    syncCoverSizeVars();
    expect(v("--cover-min")).toBe("128px");
    expect(v("--cover-w")).toBe("136.25px");
  });

  it("ignores junk", () => {
    localStorage.setItem(COVER_SIZE_KEY, "abc");
    localStorage.setItem(COVER_WIDTH_KEY, "5000");
    syncCoverSizeVars();
    expect(v("--cover-min")).toBe("");
    expect(v("--cover-w")).toBe("");
    expect(parseCoverSize("abc")).toBeNull();
    expect(parseCoverSize("5000")).toBeNull();
    expect(parseCoverSize("")).toBeNull();
    expect(parseCoverSize(null)).toBeNull();
    expect(parseCoverSize("200")).toBe(200);
  });

  it("starts from the old library slider until a size is chosen", () => {
    localStorage.setItem(LEGACY_LIBRARY_SIZE_KEY, "200");
    syncCoverSizeVars();
    expect(v("--cover-min")).toBe("200px");
    localStorage.setItem(COVER_SIZE_KEY, "140");
    syncCoverSizeVars();
    expect(v("--cover-min")).toBe("140px");
  });

  it("drops variables whose keys are gone", () => {
    localStorage.setItem(COVER_SIZE_KEY, "200");
    localStorage.setItem(COVER_WIDTH_KEY, "210");
    syncCoverSizeVars();
    expect(v("--cover-min")).toBe("200px");
    localStorage.clear();
    syncCoverSizeVars();
    expect(v("--cover-min")).toBe("");
    expect(v("--cover-w")).toBe("");
  });

  it("runs standalone as the inline bootstrap", () => {
    localStorage.setItem(COVER_SIZE_KEY, "180");
    localStorage.setItem(COVER_WIDTH_KEY, "186.66");
    // Same as the browser running the <script> body: no module scope.
    new Function(COVER_SIZE_BOOTSTRAP)();
    expect(v("--cover-min")).toBe("180px");
    expect(v("--cover-w")).toBe("186.66px");
  });

  it("builds templates that fall back to the default", () => {
    // Fill-the-row tracks: the `1fr` maximum is what keeps the row's end
    // from going blank; the `min(…, 50% - gap/2)` is the two-column floor.
    expect(COVER_GRID_COLUMNS).toBe(
      "repeat(auto-fill, minmax(min(var(--cover-min, 160px), calc(50% - 0.5rem)), 1fr))",
    );
    expect(COVER_WIDTH).toBe("var(--cover-w, var(--cover-min, 160px))");
  });
});

describe("coverColumnWidth", () => {
  it("is the stretched column of the fill-the-row grid", () => {
    // 4 columns of at least 200 fit 1000px; they share the 952px left
    // after three gaps.
    expect(coverColumnWidth(1000, 200)).toBe(238);
    // Floored to 1/100 px so the columns never add up past the row.
    expect(coverColumnWidth(1200, 160)).toBe(186.66);
  });

  it("falls back to the target before measuring", () => {
    expect(coverColumnWidth(0, 160)).toBe(160);
  });
});

describe("coverStops", () => {
  it("offers the column counts that give a sensible cover", () => {
    // 1200px: 4 per row is 288px wide, 10 per row is 105.6px.
    expect(coverStops(1200)).toEqual([4, 5, 6, 7, 8, 9, 10]);
    // A phone (390px screen, 16px gutters): two or three.
    expect(coverStops(358)).toEqual([2, 3]);
    expect(coverStops(0)).toEqual([]);
  });

  it("keeps every stop's cover width in range", () => {
    for (let w = 320; w <= 2600; w += 41) {
      const stops = coverStops(w);
      expect(stops.length).toBeGreaterThan(0);
      for (const n of stops) {
        const width = computeColumnWidth(w, n);
        expect(width).toBeGreaterThanOrEqual(COVER_STOP_MIN_PX);
        expect(width).toBeLessThanOrEqual(COVER_STOP_MAX_PX);
      }
    }
  });

  it("never offers one column where the two-column floor applies", () => {
    expect(coverStops(280)).toEqual([2]);
    // A sliver narrower than two minimum cards does get its single card.
    expect(coverStops(150)).toEqual([1]);
  });

  it("includes the current count even when it is out of range", () => {
    expect(coverStops(1200, 3)).toEqual([3, 4, 5, 6, 7, 8, 9, 10]);
    expect(coverStops(1200, 6)).toEqual([4, 5, 6, 7, 8, 9, 10]);
  });
});

describe("coverTargetForColumns", () => {
  it("stores the middle of the band that yields the count", () => {
    // 8 per row of 1200px is 136px, 9 per row 119.1px.
    expect(coverTargetForColumns(1200, 8)).toBe(128);
    expect(coverTargetForColumns(0, 8)).toBe(160);
  });

  it("reproduces the chosen count, and survives a scrollbar's width", () => {
    for (let w = 320; w <= 2600; w += 41) {
      for (const n of coverStops(w)) {
        const target = coverTargetForColumns(w, n);
        expect(computeColumnsPerRow(w, target)).toBe(n);
        expect(computeColumnsPerRow(w - 6, target)).toBe(n);
        expect(computeColumnsPerRow(w + 6, target)).toBe(n);
      }
    }
  });
});
