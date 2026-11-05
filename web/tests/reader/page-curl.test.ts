import { describe, expect, it } from "vitest";

import {
  liftSide,
  planCurl,
  spineX,
  uprightAngle,
  type PageSnap,
} from "@/lib/reader/page-curl";

const snap = (src: string, left = 0, width = 100): PageSnap => ({
  rect: { left, top: 0, width, height: 150 },
  src,
});

describe("liftSide", () => {
  it("turns the right page forward in LTR and the left page in RTL", () => {
    expect(liftSide("ltr", true)).toBe("right");
    expect(liftSide("rtl", true)).toBe("left");
  });
  it("reverses going back", () => {
    expect(liftSide("ltr", false)).toBe("left");
    expect(liftSide("rtl", false)).toBe("right");
  });
});

describe("uprightAngle / spineX", () => {
  it("hinges a right-hand leaf on its left edge and swings it left", () => {
    expect(uprightAngle("right")).toBe(-90);
    expect(spineX("right", snap("a", 50).rect)).toBe(50);
  });
  it("hinges a left-hand leaf on its right edge and swings it right", () => {
    expect(uprightAngle("left")).toBe(90);
    expect(spineX("left", snap("a", 50).rect)).toBe(150);
  });
});

describe("planCurl — single page", () => {
  const from = [snap("old")];
  const to = [snap("new")];

  it("LTR forward lifts the old page off its left-edge spine", () => {
    expect(planCurl({ direction: "ltr", forward: true, from, to })).toEqual({
      lift: { side: "right", page: from[0] },
      land: null,
    });
  });

  it("LTR back lands the new page over the old one", () => {
    expect(planCurl({ direction: "ltr", forward: false, from, to })).toEqual({
      lift: null,
      land: { side: "right", page: to[0], under: from[0] },
    });
  });

  it("RTL forward curls the other way (spine on the right)", () => {
    expect(planCurl({ direction: "rtl", forward: true, from, to })).toEqual({
      lift: { side: "left", page: from[0] },
      land: null,
    });
  });

  it("RTL back lands from the right-hand spine", () => {
    expect(planCurl({ direction: "rtl", forward: false, from, to })).toEqual({
      lift: null,
      land: { side: "left", page: to[0], under: from[0] },
    });
  });
});

describe("planCurl — spreads", () => {
  // Visual order: [left pane, right pane].
  const from = [snap("oldL", 0), snap("oldR", 100)];
  const to = [snap("newL", 0), snap("newR", 100)];

  it("LTR forward lifts the old right page and lands the new left page", () => {
    expect(planCurl({ direction: "ltr", forward: true, from, to })).toEqual({
      lift: { side: "right", page: from[1] },
      land: { side: "left", page: to[0], under: from[0] },
    });
  });

  it("RTL forward lifts the old left page and lands the new right page", () => {
    expect(planCurl({ direction: "rtl", forward: true, from, to })).toEqual({
      lift: { side: "left", page: from[0] },
      land: { side: "right", page: to[1], under: from[1] },
    });
  });

  it("LTR back mirrors forward", () => {
    expect(planCurl({ direction: "ltr", forward: false, from, to })).toEqual({
      lift: { side: "left", page: from[0] },
      land: { side: "right", page: to[1], under: from[1] },
    });
  });
});

describe("planCurl — solo cover ↔ pair", () => {
  it("lifts the outgoing page only", () => {
    const cover = [snap("cover", 50)];
    const pair = [snap("L", 0), snap("R", 100)];
    expect(
      planCurl({ direction: "ltr", forward: true, from: cover, to: pair }),
    ).toEqual({ lift: { side: "right", page: cover[0] }, land: null });
    expect(
      planCurl({ direction: "ltr", forward: false, from: pair, to: cover }),
    ).toEqual({ lift: { side: "left", page: pair[0] }, land: null });
  });

  it("returns null with nothing on screen", () => {
    expect(
      planCurl({ direction: "ltr", forward: true, from: [], to: [snap("a")] }),
    ).toBeNull();
  });
});
