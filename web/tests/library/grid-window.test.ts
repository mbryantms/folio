import { describe, expect, it } from "vitest";

import {
  computeColumnsPerRow,
  computeColumnWidth,
  computeRowCount,
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

  it("matches the auto-fill minmax inversion (gap=16)", () => {
    // One 160 card: needs 160. Two: 160+16+160 = 336.
    expect(computeColumnsPerRow(160, 160)).toBe(1);
    expect(computeColumnsPerRow(335, 160)).toBe(1);
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
  it("splits leftover width after gaps evenly", () => {
    // 864 wide, 5 cols, 4 gaps*16 = 64 → 800/5 = 160.
    expect(computeColumnWidth(864, 5)).toBe(160);
  });

  it("equals full width for a single column (no gaps)", () => {
    expect(computeColumnWidth(500, 1)).toBe(500);
  });

  it("guards zero width / zero columns", () => {
    expect(computeColumnWidth(0, 4)).toBe(0);
    expect(computeColumnWidth(500, 0)).toBe(0);
  });
});

describe("estimateRowHeight", () => {
  it("is cover (1.5×width) + text block + row gap", () => {
    // 160 wide → cover 240; + 56 text + 16 gap = 312.
    expect(estimateRowHeight(160, 56)).toBe(312);
  });
});
