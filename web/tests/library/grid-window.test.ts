import { describe, expect, it } from "vitest";

import {
  computeColumnsPerRow,
  computeColumnWidth,
  computeRowCount,
  effectiveColumnWidth,
  estimateRowHeight,
  rowItemRange,
  GRID_GAP_PX,
} from "@/lib/library/grid-window";

describe("computeColumnsPerRow", () => {
  it("returns 1 for zero/negative width (pre-measure guard)", () => {
    expect(computeColumnsPerRow(0, 160)).toBe(1);
    expect(computeColumnsPerRow(-50, 160)).toBe(1);
  });

  it("never drops below a single column even in a sliver", () => {
    expect(computeColumnsPerRow(80, 160)).toBe(1);
  });

  it("floors at two columns on phone-width rows (phone-layout.md)", () => {
    // 390px phone: 358 content, 350 with the A–Z gutter reserved.
    // 375px phone: 343 / 335. All stay two-up at the default 160 and
    // at a 280 target — the effective card width is capped at half the
    // row, matching `COVER_GRID_COLUMNS`'s `min(<target>, 50% - 0.5rem)`
    // tracks.
    for (const w of [358, 350, 343, 335]) {
      expect(computeColumnsPerRow(w, 160)).toBe(2);
      expect(computeColumnsPerRow(w, 280)).toBe(2);
    }
    // The floor stops at 96px cards: a 200px row is two 92s → one card.
    expect(computeColumnsPerRow(200, 160)).toBe(1);
    expect(computeColumnsPerRow(208, 160)).toBe(2);
    // Wide containers are untouched by the floor.
    expect(computeColumnsPerRow(1200, 160)).toBe(6);
  });

  it("matches the auto-fill column-count inversion (gap=16)", () => {
    // One 160 card: needs 160 (half of 160 is below the 96px floor).
    expect(computeColumnsPerRow(160, 160)).toBe(1);
    // Two: 160+16+160 = 336 (the floor already grants two from 208).
    expect(computeColumnsPerRow(336, 160)).toBe(2);
    // Five 160 cards: 5*160 + 4*16 = 864.
    expect(computeColumnsPerRow(864, 160)).toBe(5);
    expect(computeColumnsPerRow(863, 160)).toBe(4);
  });

  it("uses the default gap constant", () => {
    expect(computeColumnsPerRow(336, 160, GRID_GAP_PX)).toBe(
      computeColumnsPerRow(336, 160),
    );
  });
});

describe("computeRowCount", () => {
  it("is zero for an empty list", () => {
    expect(computeRowCount(0, 4)).toBe(0);
  });

  it("ceils a partial last row", () => {
    expect(computeRowCount(7, 4)).toBe(2);
    expect(computeRowCount(8, 4)).toBe(2);
    expect(computeRowCount(9, 4)).toBe(3);
  });

  it("is one row when items fit a single row", () => {
    expect(computeRowCount(3, 4)).toBe(1);
  });
});

describe("rowItemRange", () => {
  it("slices full rows", () => {
    expect(rowItemRange(0, 4, 10)).toEqual({ start: 0, end: 4 });
    expect(rowItemRange(1, 4, 10)).toEqual({ start: 4, end: 8 });
  });

  it("clamps the partial last row to itemCount", () => {
    // 7 items, 4 cols → row 1 holds indices [4,7).
    expect(rowItemRange(1, 4, 7)).toEqual({ start: 4, end: 7 });
  });
});

describe("computeColumnWidth", () => {
  it("stretches the columns to fill the row (1fr tracks)", () => {
    // 1000 wide, 4 columns: the three 16px gaps leave 952 to share, so
    // each column is 238 and the row never ends in a blank strip.
    expect(computeColumnWidth(1000, 4)).toBe(238);
    // Five columns in 864 = 5 × 160 + 4 gaps: nothing left over to share.
    expect(computeColumnWidth(864, 5)).toBe(160);
    // A single column takes the whole row.
    expect(computeColumnWidth(200, 1)).toBe(200);
  });

  it("halves the row under the two-column phone floor", () => {
    // 390 phone, two columns: half the row minus the one gap.
    expect(computeColumnWidth(390, 2)).toBe((390 - 16) / 2);
  });

  it("subtracts the caller's gap when one is given", () => {
    expect(computeColumnWidth(1000, 4, 8)).toBe((1000 - 3 * 8) / 4);
    expect(computeColumnWidth(1000, 4, GRID_GAP_PX)).toBe(
      computeColumnWidth(1000, 4),
    );
  });

  it("guards zero width / zero columns", () => {
    expect(computeColumnWidth(0, 4)).toBe(0);
    expect(computeColumnWidth(500, 0)).toBe(0);
  });
});

describe("effectiveColumnWidth", () => {
  it("is the stretched width of the columns the target fits", () => {
    // A 200 target fits 4 columns in 1000 (4 × 200 + 3 gaps = 848), and
    // those four share the row: 238 each, never the bare 200.
    expect(effectiveColumnWidth(1000, 200)).toBe(238);
    expect(effectiveColumnWidth(864, 160)).toBe(160);
    // One fewer pixel drops a column, and the remaining four widen.
    expect(effectiveColumnWidth(863, 160)).toBe((863 - 3 * 16) / 4);
  });

  it("is never narrower than half a phone row", () => {
    // 358px of content at a 280 target: the two-column floor applies.
    expect(effectiveColumnWidth(358, 280)).toBe((358 - 16) / 2);
  });

  it("falls back to the target before the first measurement", () => {
    expect(effectiveColumnWidth(0, 160)).toBe(160);
  });
});

describe("estimateRowHeight", () => {
  it("is cover (1.5×width) + text block + row gap", () => {
    // 160 wide → cover 240; + 56 text + 16 gap = 312.
    expect(estimateRowHeight(160, 56)).toBe(312);
  });
});
