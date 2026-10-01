import { describe, expect, it } from "vitest";

import {
  DOUBLE_TAP_DIST,
  DOUBLE_TAP_MS,
  MAX_ZOOM,
  MIN_ZOOM,
  ZOOM_IDENTITY,
  clampPan,
  clampScale,
  clampZoomPan,
  isDoubleTap,
  nextZoomStep,
  sameZoom,
  wheelZoomFactor,
  zoomAboutPoint,
  zoomAfterPageTurn,
  zoomOriginPercent,
  type ZoomState,
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
      isDoubleTap(
        { t: 0, x: 10, y: 10 },
        { t: DOUBLE_TAP_MS - 1, x: 20, y: 12 },
      ),
    ).toBe(true);
  });

  it("rejects too-slow or too-far", () => {
    expect(
      isDoubleTap(
        { t: 0, x: 10, y: 10 },
        { t: DOUBLE_TAP_MS + 50, x: 10, y: 10 },
      ),
    ).toBe(false);
    expect(
      isDoubleTap(
        { t: 0, x: 10, y: 10 },
        { t: 100, x: 10 + DOUBLE_TAP_DIST + 5, y: 10 },
      ),
    ).toBe(false);
  });
});

// ---- WP-4.2: continuous zoom, origin-aware clamp, page-turn carry ----

/** Screen x/y (relative to the surface's layout box) of local point `p`
 *  under `translate(T) scale(s)` about origin `O` — the transform the
 *  reader renders. */
function project(
  z: ZoomState,
  p: { x: number; y: number },
  box: { w: number; h: number },
) {
  const ox = (z.origin.x / 100) * box.w;
  const oy = (z.origin.y / 100) * box.h;
  return {
    x: z.offset.x + ox + z.scale * (p.x - ox),
    y: z.offset.y + oy + z.scale * (p.y - oy),
  };
}

describe("wheelZoomFactor", () => {
  it("zooms in on negative delta (scroll up / pinch out) and out on positive", () => {
    expect(wheelZoomFactor(-10)).toBeGreaterThan(1);
    expect(wheelZoomFactor(10)).toBeLessThan(1);
    expect(wheelZoomFactor(0)).toBe(1);
  });

  it("is symmetric so in-then-out returns to the start", () => {
    expect(wheelZoomFactor(-7) * wheelZoomFactor(7)).toBeCloseTo(1, 10);
  });

  it("caps a single mouse-wheel notch so one event can't jump the range", () => {
    expect(wheelZoomFactor(-100)).toBe(wheelZoomFactor(-1000));
    expect(wheelZoomFactor(-100)).toBeLessThan(1.5);
  });

  it("normalizes line- and page-mode deltas to pixels", () => {
    expect(wheelZoomFactor(1, 1)).toBe(wheelZoomFactor(16, 0));
    expect(wheelZoomFactor(-1, 2)).toBe(wheelZoomFactor(-800, 0));
  });
});

describe("clampScale", () => {
  it("clamps into [MIN_ZOOM, MAX_ZOOM] and rejects non-finite input", () => {
    expect(clampScale(0.2)).toBe(MIN_ZOOM);
    expect(clampScale(99)).toBe(MAX_ZOOM);
    expect(clampScale(1.7)).toBe(1.7);
    expect(clampScale(Number.NaN)).toBe(MIN_ZOOM);
  });
});

describe("clampZoomPan", () => {
  const box = { w: 1000, h: 1500 };

  it("matches the centered clamp for a 50% origin", () => {
    const origin = { x: 50, y: 50 };
    expect(clampZoomPan({ x: 9999, y: -9999 }, 2, origin, box)).toEqual({
      x: 500,
      y: -750,
    });
  });

  it("is asymmetric for a top-left origin (content only extends right/down)", () => {
    const origin = { x: 0, y: 0 };
    expect(clampZoomPan({ x: 50, y: 50 }, 2, origin, box)).toEqual({
      x: 0,
      y: 0,
    });
    expect(clampZoomPan({ x: -9999, y: -9999 }, 2, origin, box)).toEqual({
      x: -1000,
      y: -1500,
    });
  });

  it("pins to zero at 1x", () => {
    expect(clampZoomPan({ x: 40, y: 40 }, 1, { x: 50, y: 50 }, box)).toEqual({
      x: 0,
      y: 0,
    });
  });

  it("keeps the scaled page covering the visible box for any origin", () => {
    for (const origin of [
      { x: 0, y: 0 },
      { x: 100, y: 0 },
      { x: 30, y: 70 },
    ]) {
      for (const raw of [
        { x: 5000, y: 5000 },
        { x: -5000, y: -5000 },
      ]) {
        const z: ZoomState = {
          scale: 2.5,
          origin,
          offset: clampZoomPan(raw, 2.5, origin, box),
        };
        const topLeft = project(z, { x: 0, y: 0 }, box);
        const bottomRight = project(z, { x: box.w, y: box.h }, box);
        expect(topLeft.x).toBeLessThanOrEqual(1e-9);
        expect(topLeft.y).toBeLessThanOrEqual(1e-9);
        expect(bottomRight.x).toBeGreaterThanOrEqual(box.w - 1e-9);
        expect(bottomRight.y).toBeGreaterThanOrEqual(box.h - 1e-9);
      }
    }
  });
});

describe("zoomAboutPoint", () => {
  const box = { w: 1000, h: 1500 };

  it("keeps the content under the cursor stationary", () => {
    const cursor = { x: 300, y: 400 };
    const z1 = zoomAboutPoint(ZOOM_IDENTITY, 1.5, cursor, box);
    expect(z1.scale).toBe(1.5);
    // The local point that was under the cursor at 1x is still there.
    const p = project(z1, cursor, box);
    expect(p.x).toBeCloseTo(cursor.x, 6);
    expect(p.y).toBeCloseTo(cursor.y, 6);
    // …and again from an already-zoomed, panned state.
    const cursor2 = { x: 700, y: 900 };
    const before = project(z1, { x: 600, y: 800 }, box);
    const localUnderCursor = {
      x: 600 + (cursor2.x - before.x) / z1.scale,
      y: 800 + (cursor2.y - before.y) / z1.scale,
    };
    const z2 = zoomAboutPoint(z1, 2.2, cursor2, box);
    const after = project(z2, localUnderCursor, box);
    expect(after.x).toBeCloseTo(cursor2.x, 6);
    expect(after.y).toBeCloseTo(cursor2.y, 6);
  });

  it("works with a non-center origin (after a double-tap)", () => {
    const z: ZoomState = {
      scale: 2,
      offset: { x: 0, y: 0 },
      origin: { x: 20, y: 30 },
    };
    const cursor = { x: 500, y: 500 };
    const local = {
      x: 200 + (cursor.x - 200) / 2,
      y: 450 + (cursor.y - 450) / 2,
    };
    const next = zoomAboutPoint(z, 2.5, cursor, box);
    expect(next.origin).toEqual(z.origin);
    const p = project(next, local, box);
    expect(p.x).toBeCloseTo(cursor.x, 6);
    expect(p.y).toBeCloseTo(cursor.y, 6);
  });

  it("snaps to identity when zooming out to ~1x", () => {
    const z = zoomAboutPoint(ZOOM_IDENTITY, 2, { x: 10, y: 10 }, box);
    expect(zoomAboutPoint(z, 1.005, { x: 10, y: 10 }, box)).toEqual(
      ZOOM_IDENTITY,
    );
    expect(zoomAboutPoint(z, 0.5, { x: 10, y: 10 }, box)).toEqual(
      ZOOM_IDENTITY,
    );
  });

  it("clamps the scale to MAX_ZOOM", () => {
    expect(zoomAboutPoint(ZOOM_IDENTITY, 50, { x: 0, y: 0 }, box).scale).toBe(
      MAX_ZOOM,
    );
  });
});

describe("zoomAfterPageTurn", () => {
  const zoomed: ZoomState = {
    scale: 2,
    offset: { x: -120, y: -300 },
    origin: { x: 40, y: 60 },
  };

  it("resets to 1x when the keep-zoom preference is off", () => {
    expect(zoomAfterPageTurn(zoomed, false, "ltr")).toEqual(ZOOM_IDENTITY);
  });

  it("carries the scale and lands on the top-left corner in LTR", () => {
    expect(zoomAfterPageTurn(zoomed, true, "ltr")).toEqual({
      scale: 2,
      offset: { x: 0, y: 0 },
      origin: { x: 0, y: 0 },
    });
  });

  it("lands on the top-right corner in RTL", () => {
    expect(zoomAfterPageTurn(zoomed, true, "rtl").origin).toEqual({
      x: 100,
      y: 0,
    });
  });

  it("stays at identity when not zoomed", () => {
    expect(zoomAfterPageTurn(ZOOM_IDENTITY, true, "ltr")).toEqual(
      ZOOM_IDENTITY,
    );
  });
});

describe("sameZoom", () => {
  it("compares structurally", () => {
    expect(sameZoom(ZOOM_IDENTITY, { ...ZOOM_IDENTITY })).toBe(true);
    expect(sameZoom(ZOOM_IDENTITY, { ...ZOOM_IDENTITY, scale: 2 })).toBe(false);
  });
});
