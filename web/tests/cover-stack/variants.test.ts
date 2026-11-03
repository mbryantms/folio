import { describe, expect, it } from "vitest";

import {
  BACK_COVERS,
  COVER_STACK_STYLES,
  STACK_ASPECT,
  boundsFor,
  cornersFor,
  isCoverStackStyle,
  layoutFor,
} from "@/lib/cover-stack/variants";

describe("cover-stack variants", () => {
  it.each(COVER_STACK_STYLES)(
    "%s: every cover, rest and fanned, stays inside the stack box",
    (style) => {
      const b = boundsFor(style);
      const span = b.maxX - b.minX;
      const boxH = STACK_ASPECT * span;
      const padY = (boxH - (b.maxY - b.minY)) / 2;
      for (let d = 0; d <= BACK_COVERS; d++) {
        for (const fanned of [false, true]) {
          for (const [x, y] of cornersFor(style, d, fanned)) {
            const bx = x - b.minX;
            const by = y - b.minY + padY;
            expect(bx).toBeGreaterThanOrEqual(-1e-9);
            expect(bx).toBeLessThanOrEqual(span + 1e-9);
            expect(by).toBeGreaterThanOrEqual(-1e-9);
            expect(by).toBeLessThanOrEqual(boxH + 1e-9);
          }
        }
      }
    },
  );

  it("keeps the front cover near 70% of the card in every style", () => {
    for (const style of COVER_STACK_STYLES) {
      const pct = layoutFor(style).coverWidthPct;
      expect(pct, style).toBeGreaterThan(66);
      expect(pct, style).toBeLessThan(74);
    }
  });

  it("draws back-most first with matching transform function lists", () => {
    for (const style of COVER_STACK_STYLES) {
      const { layers } = layoutFor(style);
      expect(layers.map((l) => l.depth)).toEqual([3, 2, 1, 0]);
      for (const l of layers) {
        const fns = (t: string) => t.match(/[a-z]+\(/g);
        expect(fns(l.rest)).toEqual(fns(l.fan));
      }
    }
  });

  it("validates style names", () => {
    expect(isCoverStackStyle("pickup")).toBe(true);
    expect(isCoverStackStyle("spiral")).toBe(false);
    expect(isCoverStackStyle(null)).toBe(false);
  });
});
