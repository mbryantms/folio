import { describe, expect, it } from "vitest";

import {
  DOUBLE_TAP_DIST,
  DOUBLE_TAP_MS,
  MAX_ZOOM,
  MIN_ZOOM,
  clampPan,
  isDoubleTap,
  nextZoomStep,
  zoomOriginPercent,
} from "@/lib/reader/zoom";

describe("nextZoomStep", () => {
  it("walks the ladder in", () => {
    expect(nextZoomStep(1, "in")).toBe(1.5);
    expect(nextZoomStep(1.5, "in")).toBe(2);
    expect(nextZoomStep(2, "in")).toBe(3);
  });

  it("caps at max when zooming in past the top", () => {
    expect(nextZoomStep(3, "in")).toBe(MAX_ZOOM);
    expect(nextZoomStep(5, "in")).toBe(MAX_ZOOM);
  });

  it("walks the ladder out and floors at 1", () => {
    expect(nextZoomStep(3, "out")).toBe(2);
    expect(nextZoomStep(1.5, "out")).toBe(MIN_ZOOM);
    expect(nextZoomStep(1, "out")).toBe(MIN_ZOOM);
  });

  it("snaps a between-steps value to the next rung", () => {
    expect(nextZoomStep(1.8, "in")).toBe(2);
    expect(nextZoomStep(1.8, "out")).toBe(1.5);
  });
});

describe("clampPan", () => {
  it("pins to center when content fits the container", () => {
    expect(
      clampPan({ x: 100, y: 100 }, { w: 400, h: 600 }, { w: 400, h: 600 }),
    ).toEqual({ x: 0, y: 0 });
  });

  it("clamps within the (content-container)/2 envelope (zoom 2×)", () => {
    // content 800×1200 in a 400×600 box → maxX 200, maxY 300.
    expect(
      clampPan({ x: 500, y: -500 }, { w: 800, h: 1200 }, { w: 400, h: 600 }),
    ).toEqual({ x: 200, y: -300 });
  });

  it("allows horizontal pan but pins vertical for an overflowing fit=height page", () => {
    // Wide page (1000) in a 400 box, same height → pan X up to 300, Y pinned.
    expect(
      clampPan({ x: 999, y: 50 }, { w: 1000, h: 600 }, { w: 400, h: 600 }),
    ).toEqual({ x: 300, y: 0 });
  });

  it("leaves in-bounds offsets untouched", () => {
    expect(
      clampPan({ x: 50, y: -40 }, { w: 800, h: 1200 }, { w: 400, h: 600 }),
    ).toEqual({ x: 50, y: -40 });
  });
});

describe("zoomOriginPercent", () => {
  it("maps a tap point to percentages", () => {
    expect(zoomOriginPercent(100, 300, { w: 400, h: 600 })).toEqual({
      x: 25,
      y: 50,
    });
  });

  it("clamps out-of-rect taps and guards zero dims", () => {
    expect(zoomOriginPercent(800, -10, { w: 400, h: 600 })).toEqual({
      x: 100,
      y: 0,
    });
    expect(zoomOriginPercent(10, 10, { w: 0, h: 0 })).toEqual({ x: 50, y: 50 });
  });
});

describe("isDoubleTap", () => {
  it("is false with no prior tap", () => {
    expect(isDoubleTap(null, { t: 100, x: 10, y: 10 })).toBe(false);
  });

  it("recognizes a close, quick second tap", () => {
    expect(
      isDoubleTap({ t: 0, x: 10, y: 10 }, { t: DOUBLE_TAP_MS - 1, x: 20, y: 12 }),
    ).toBe(true);
  });

  it("rejects too-slow or too-far", () => {
    expect(
      isDoubleTap({ t: 0, x: 10, y: 10 }, { t: DOUBLE_TAP_MS + 50, x: 10, y: 10 }),
    ).toBe(false);
    expect(
      isDoubleTap(
        { t: 0, x: 10, y: 10 },
        { t: 100, x: 10 + DOUBLE_TAP_DIST + 5, y: 10 },
      ),
    ).toBe(false);
  });
});
