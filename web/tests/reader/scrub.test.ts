/**
 * Page scrubber geometry + keys (`lib/reader/scrub.ts`), including the
 * right-to-left bar.
 */
import { describe, expect, it } from "vitest";
import {
  scrubFraction,
  scrubKeyTarget,
  scrubPageAt,
  scrubPageX,
  scrubPreviewLeft,
} from "@/lib/reader/scrub";

const rect = { left: 100, width: 1000 };

describe("scrubPageAt", () => {
  it("maps the bar's ends and middle to pages (LTR)", () => {
    expect(scrubPageAt(100, rect, 11, "ltr")).toBe(0);
    expect(scrubPageAt(600, rect, 11, "ltr")).toBe(5);
    expect(scrubPageAt(1100, rect, 11, "ltr")).toBe(10);
  });

  it("clamps pointers past the ends", () => {
    expect(scrubPageAt(-50, rect, 11, "ltr")).toBe(0);
    expect(scrubPageAt(5000, rect, 11, "ltr")).toBe(10);
  });

  it("runs right to left in RTL", () => {
    expect(scrubPageAt(1100, rect, 11, "rtl")).toBe(0);
    expect(scrubPageAt(100, rect, 11, "rtl")).toBe(10);
    expect(scrubPageAt(200, rect, 11, "rtl")).toBe(9);
  });

  it("degenerates to page 0 for single-page issues / empty bars", () => {
    expect(scrubPageAt(500, rect, 1, "ltr")).toBe(0);
    expect(scrubPageAt(500, { left: 0, width: 0 }, 10, "ltr")).toBe(0);
  });
});

describe("scrubFraction / scrubPageX", () => {
  it("round-trips with scrubPageAt", () => {
    for (const dir of ["ltr", "rtl"] as const) {
      for (let p = 0; p < 35; p += 1) {
        const x = rect.left + scrubPageX(p, 35, rect.width, dir);
        expect(scrubPageAt(x, rect, 35, dir)).toBe(p);
      }
    }
  });

  it("puts page 1 at the reading start edge", () => {
    expect(scrubFraction(0, 10)).toBe(0);
    expect(scrubFraction(9, 10)).toBe(1);
    expect(scrubPageX(0, 10, 500, "ltr")).toBe(0);
    expect(scrubPageX(0, 10, 500, "rtl")).toBe(500);
  });
});

describe("scrubPreviewLeft", () => {
  it("centres the card on the knob, kept inside the bar", () => {
    expect(scrubPreviewLeft(500, 100, 1000)).toBe(450);
    expect(scrubPreviewLeft(0, 100, 1000)).toBe(8);
    expect(scrubPreviewLeft(1000, 100, 1000)).toBe(892);
  });
});

describe("scrubKeyTarget", () => {
  const one = (p: number) => ({ first: p, last: p });

  it("steps with arrows and clamps (LTR)", () => {
    expect(scrubKeyTarget("ArrowRight", one(3), 10, "ltr")).toBe(4);
    expect(scrubKeyTarget("ArrowLeft", one(3), 10, "ltr")).toBe(2);
    expect(scrubKeyTarget("ArrowLeft", one(0), 10, "ltr")).toBe(0);
    expect(scrubKeyTarget("ArrowUp", one(9), 10, "ltr")).toBe(9);
  });

  it("flips horizontal arrows in RTL but not vertical ones", () => {
    expect(scrubKeyTarget("ArrowLeft", one(3), 10, "rtl")).toBe(4);
    expect(scrubKeyTarget("ArrowRight", one(3), 10, "rtl")).toBe(2);
    expect(scrubKeyTarget("ArrowUp", one(3), 10, "rtl")).toBe(4);
    expect(scrubKeyTarget("ArrowDown", one(3), 10, "rtl")).toBe(2);
  });

  it("jumps with Home / End / PageUp / PageDown", () => {
    expect(scrubKeyTarget("Home", one(5), 40, "ltr")).toBe(0);
    expect(scrubKeyTarget("End", one(5), 40, "ltr")).toBe(39);
    expect(scrubKeyTarget("PageUp", one(5), 40, "ltr")).toBe(9);
    expect(scrubKeyTarget("PageDown", one(5), 40, "ltr")).toBe(1);
  });

  it("leaves a double-page spread in one press", () => {
    const spread = { first: 3, last: 4 };
    expect(scrubKeyTarget("ArrowRight", spread, 10, "ltr")).toBe(5);
    expect(scrubKeyTarget("ArrowLeft", spread, 10, "ltr")).toBe(2);
  });

  it("ignores other keys", () => {
    expect(scrubKeyTarget("a", one(3), 10, "ltr")).toBeNull();
    expect(scrubKeyTarget("Enter", one(3), 10, "ltr")).toBeNull();
  });
});
