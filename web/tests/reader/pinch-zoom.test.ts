/**
 * Pure math behind the touch pinch → transform zoom (audit N-R9,
 * remediation C5). The DOM wiring (touch events on the gesture
 * container) is covered in tests/dom/reader-pinch-zoom.test.tsx.
 */
import { describe, expect, it } from "vitest";
import { applyPinch, pinchGeometry } from "@/lib/reader/pinch-zoom";
import {
  MAX_ZOOM,
  ZOOM_IDENTITY,
  readerTouchAction,
  type ZoomState,
} from "@/lib/reader/zoom";
import { baseViewport } from "@/lib/viewport";

const BOX = { w: 400, h: 600 };

/** Screen position of surface point `p` under `z` (translate then scale
 *  about the % origin) — the invariant a zoom-about-point must hold. */
function screenPoint(z: ZoomState, p: { x: number; y: number }) {
  const ox = (z.origin.x / 100) * BOX.w;
  const oy = (z.origin.y / 100) * BOX.h;
  return {
    x: z.offset.x + ox + z.scale * (p.x - ox),
    y: z.offset.y + oy + z.scale * (p.y - oy),
  };
}

describe("pinchGeometry", () => {
  it("returns the midpoint and finger distance", () => {
    const g = pinchGeometry({ x: 100, y: 200 }, { x: 200, y: 200 });
    expect(g.mid).toEqual({ x: 150, y: 200 });
    expect(g.dist).toBe(100);
  });
});

describe("applyPinch", () => {
  it("fingers moving apart zoom in about the midpoint", () => {
    const from = pinchGeometry({ x: 100, y: 250 }, { x: 200, y: 250 });
    const to = pinchGeometry({ x: 75, y: 250 }, { x: 225, y: 250 });
    const z = applyPinch(ZOOM_IDENTITY, from, to, BOX);
    expect(z.scale).toBeCloseTo(1.5, 6);
    // The art under the fingers' midpoint has not moved.
    const s = screenPoint(z, from.mid);
    expect(s.x).toBeCloseTo(from.mid.x, 6);
    expect(s.y).toBeCloseTo(from.mid.y, 6);
  });

  it("fingers moving together zoom out, and back to the exact identity", () => {
    const start = applyPinch(
      ZOOM_IDENTITY,
      pinchGeometry({ x: 100, y: 300 }, { x: 200, y: 300 }),
      pinchGeometry({ x: 50, y: 300 }, { x: 250, y: 300 }),
      BOX,
    );
    expect(start.scale).toBeCloseTo(2, 6);
    const from = pinchGeometry({ x: 50, y: 300 }, { x: 250, y: 300 });
    const narrower = applyPinch(
      start,
      from,
      pinchGeometry({ x: 80, y: 300 }, { x: 220, y: 300 }),
      BOX,
    );
    expect(narrower.scale).toBeCloseTo(1.4, 6);
    expect(narrower.scale).toBeLessThan(start.scale);
    // Pinching in past 1× lands on the identity (re-centered), never 0.98×.
    const closed = applyPinch(
      start,
      from,
      pinchGeometry({ x: 120, y: 300 }, { x: 180, y: 300 }),
      BOX,
    );
    expect(closed).toEqual(ZOOM_IDENTITY);
  });

  it("is cumulative from the gesture start, so a slow zoom-in is not swallowed by the 1× snap", () => {
    const from = pinchGeometry({ x: 150, y: 300 }, { x: 250, y: 300 });
    // Each step alone is under the 1.01 snap; the cumulative pinch is not.
    let z = ZOOM_IDENTITY;
    for (let i = 1; i <= 20; i++) {
      z = applyPinch(
        ZOOM_IDENTITY,
        from,
        pinchGeometry(
          { x: 150 - i * 0.25, y: 300 },
          { x: 250 + i * 0.25, y: 300 },
        ),
        BOX,
      );
    }
    expect(z.scale).toBeCloseTo(1.1, 6);
  });

  it("follows the midpoint as a pan, clamped to the page edges", () => {
    const from = pinchGeometry({ x: 150, y: 250 }, { x: 250, y: 350 });
    const apart = pinchGeometry({ x: 100, y: 200 }, { x: 300, y: 400 });
    const zoomedOnly = applyPinch(ZOOM_IDENTITY, from, apart, BOX);
    // Same spread, both fingers moved 30px right and 20px down.
    const moved = pinchGeometry({ x: 130, y: 220 }, { x: 330, y: 420 });
    const panned = applyPinch(ZOOM_IDENTITY, from, moved, BOX);
    expect(panned.scale).toBeCloseTo(zoomedOnly.scale, 6);
    expect(panned.offset.x - zoomedOnly.offset.x).toBeCloseTo(30, 6);
    expect(panned.offset.y - zoomedOnly.offset.y).toBeCloseTo(20, 6);
    // A huge travel cannot drag the 2× page off its own edge: at most
    // (s−1)·origin = 200px on x.
    const flung = pinchGeometry({ x: 900, y: 200 }, { x: 1100, y: 400 });
    expect(applyPinch(ZOOM_IDENTITY, from, flung, BOX).offset.x).toBe(200);
  });

  it("clamps the scale to the zoom range", () => {
    const from = pinchGeometry({ x: 190, y: 300 }, { x: 210, y: 300 });
    const to = pinchGeometry({ x: 0, y: 300 }, { x: 400, y: 300 });
    expect(applyPinch(ZOOM_IDENTITY, from, to, BOX).scale).toBe(MAX_ZOOM);
  });

  it("ignores a degenerate start (both fingers on one point)", () => {
    const from = pinchGeometry({ x: 200, y: 300 }, { x: 200, y: 300 });
    const to = pinchGeometry({ x: 100, y: 300 }, { x: 300, y: 300 });
    expect(applyPinch(ZOOM_IDENTITY, from, to, BOX)).toBe(ZOOM_IDENTITY);
  });
});

describe("readerTouchAction", () => {
  it("drops pinch-zoom only while the reader's own pinch is live", () => {
    // single / double view, marker mode idle
    expect(readerTouchAction(false, true)).toBe("pan-y");
    // webtoon, or a marker being drawn: native pinch stays
    expect(readerTouchAction(false, false)).toBe("pan-y pinch-zoom");
  });

  it("hands every axis to the JS gesture while panning", () => {
    expect(readerTouchAction(true, true)).toBe("none");
    expect(readerTouchAction(true, false)).toBe("none");
  });
});

describe("viewport meta", () => {
  it("stays user-scalable app-wide (WCAG 1.4.4); the reader blocks native pinch on its own surface only", () => {
    expect(baseViewport.userScalable).toBe(true);
    expect(baseViewport.maximumScale).toBeGreaterThanOrEqual(2);
  });
});
