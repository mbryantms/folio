// @vitest-environment jsdom
/**
 * Touch-event wiring of the reader's pinch zoom (audit N-R9, C5):
 * `attachPinchZoom` on the gesture container turns a two-finger touch
 * into `ZoomState` updates about the fingers' midpoint, cancels the
 * browser's own pinch, leaves the final scale in place on release, and
 * never reacts to a single finger. The math itself is pinned in
 * tests/reader/pinch-zoom.test.ts.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { attachPinchZoom, type SurfaceBox } from "@/lib/reader/pinch-zoom";
import { ZOOM_IDENTITY, type ZoomState } from "@/lib/reader/zoom";

const SURFACE: SurfaceBox = { left: 0, top: 50, w: 400, h: 600 };

type Finger = { clientX: number; clientY: number };

function touch(el: HTMLElement, type: string, touches: Finger[]) {
  const e = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(e, "touches", { value: touches });
  el.dispatchEvent(e);
  return e;
}

function gesture(el: HTMLElement, type: string, g: Partial<GestureLike>) {
  const e = new Event(type, { bubbles: true, cancelable: true });
  Object.assign(e, g);
  el.dispatchEvent(e);
  return e;
}
type GestureLike = { scale: number; clientX: number; clientY: number };

let el: HTMLDivElement;
let zoom: ZoomState;
let enabled: boolean;
let detach: () => void;
let isPinching: () => boolean;

function attach(handleGestures = true) {
  const handle = attachPinchZoom(el, {
    enabled: () => enabled,
    getZoom: () => zoom,
    setZoom: (z) => {
      zoom = z;
    },
    getSurface: () => SURFACE,
    handleGestures,
  });
  detach = handle.detach;
  isPinching = handle.isPinching;
}

beforeEach(() => {
  el = document.createElement("div");
  document.body.appendChild(el);
  zoom = ZOOM_IDENTITY;
  enabled = true;
  attach();
});

afterEach(() => {
  detach();
  el.remove();
});

describe("attachPinchZoom", () => {
  it("two fingers moving apart zoom in about their midpoint and claim the touch", () => {
    touch(el, "touchstart", [{ clientX: 100, clientY: 350 }]);
    const start = touch(el, "touchstart", [
      { clientX: 100, clientY: 350 },
      { clientX: 200, clientY: 350 },
    ]);
    expect(start.defaultPrevented).toBe(true);
    expect(isPinching()).toBe(true);
    const move = touch(el, "touchmove", [
      { clientX: 50, clientY: 350 },
      { clientX: 250, clientY: 350 },
    ]);
    expect(move.defaultPrevented).toBe(true);
    expect(zoom.scale).toBeCloseTo(2, 6);
    // Midpoint in surface px: (150, 300) — the art there has not moved.
    const ox = (zoom.origin.x / 100) * SURFACE.w;
    const oy = (zoom.origin.y / 100) * SURFACE.h;
    expect(zoom.offset.x + ox + zoom.scale * (150 - ox)).toBeCloseTo(150, 6);
    expect(zoom.offset.y + oy + zoom.scale * (300 - oy)).toBeCloseTo(300, 6);
  });

  it("fingers moving together zoom out", () => {
    zoom = { scale: 2, offset: { x: 0, y: 0 }, origin: { x: 50, y: 50 } };
    touch(el, "touchstart", [
      { clientX: 100, clientY: 350 },
      { clientX: 300, clientY: 350 },
    ]);
    touch(el, "touchmove", [
      { clientX: 150, clientY: 350 },
      { clientX: 250, clientY: 350 },
    ]);
    expect(zoom.scale).toBeCloseTo(1, 6);
    expect(zoom).toEqual(ZOOM_IDENTITY);
  });

  it("release leaves the zoom at the final scale", () => {
    touch(el, "touchstart", [
      { clientX: 150, clientY: 350 },
      { clientX: 250, clientY: 350 },
    ]);
    touch(el, "touchmove", [
      { clientX: 125, clientY: 350 },
      { clientX: 275, clientY: 350 },
    ]);
    expect(zoom.scale).toBeCloseTo(1.5, 6);
    touch(el, "touchend", [{ clientX: 125, clientY: 350 }]);
    touch(el, "touchend", []);
    expect(zoom.scale).toBeCloseTo(1.5, 6);
    // The lift that ends the pinch still reads as pinching, so the drag
    // layer does not turn a page on it...
    expect(isPinching()).toBe(true);
    // ...and a fresh single-finger touch starts clean.
    touch(el, "touchstart", [{ clientX: 200, clientY: 350 }]);
    expect(isPinching()).toBe(false);
    // A one-finger move after the pinch does not touch the zoom.
    touch(el, "touchmove", [{ clientX: 260, clientY: 350 }]);
    expect(zoom.scale).toBeCloseTo(1.5, 6);
  });

  it("a single finger is left alone", () => {
    const start = touch(el, "touchstart", [{ clientX: 100, clientY: 350 }]);
    const move = touch(el, "touchmove", [{ clientX: 180, clientY: 350 }]);
    expect(start.defaultPrevented).toBe(false);
    expect(move.defaultPrevented).toBe(false);
    expect(zoom).toBe(ZOOM_IDENTITY);
    expect(isPinching()).toBe(false);
  });

  it("does nothing while disabled (webtoon / marker drawing): native pinch stays", () => {
    enabled = false;
    const start = touch(el, "touchstart", [
      { clientX: 100, clientY: 350 },
      { clientX: 200, clientY: 350 },
    ]);
    const move = touch(el, "touchmove", [
      { clientX: 50, clientY: 350 },
      { clientX: 250, clientY: 350 },
    ]);
    expect(start.defaultPrevented).toBe(false);
    expect(move.defaultPrevented).toBe(false);
    expect(zoom).toBe(ZOOM_IDENTITY);
    expect(isPinching()).toBe(false);
  });

  it("stands down when the first finger had already started a native scroll", () => {
    touch(el, "touchstart", [{ clientX: 100, clientY: 350 }]);
    touch(el, "touchstart", [
      { clientX: 100, clientY: 350 },
      { clientX: 200, clientY: 350 },
    ]);
    // The browser keeps its scroll: moves arrive uncancelable.
    const e = new Event("touchmove", { bubbles: true, cancelable: false });
    Object.defineProperty(e, "touches", {
      value: [
        { clientX: 50, clientY: 350 },
        { clientX: 250, clientY: 350 },
      ],
    });
    el.dispatchEvent(e);
    expect(zoom).toBe(ZOOM_IDENTITY);
    touch(el, "touchmove", [
      { clientX: 40, clientY: 350 },
      { clientX: 260, clientY: 350 },
    ]);
    expect(zoom).toBe(ZOOM_IDENTITY);
    // ...but the lift still does not read as a swipe.
    expect(isPinching()).toBe(true);
  });

  it("cancels WebKit gesture events only while enabled", () => {
    const g1 = gesture(el, "gesturestart", {
      scale: 1,
      clientX: 0,
      clientY: 0,
    });
    expect(g1.defaultPrevented).toBe(true);
    enabled = false;
    const g2 = gesture(el, "gesturestart", {
      scale: 1,
      clientX: 0,
      clientY: 0,
    });
    expect(g2.defaultPrevented).toBe(false);
  });

  it("a trackpad gesture (no touch pinch behind it) drives the zoom about the pointer", () => {
    gesture(el, "gesturestart", { scale: 1, clientX: 150, clientY: 350 });
    const change = gesture(el, "gesturechange", {
      scale: 2,
      clientX: 150,
      clientY: 350,
    });
    expect(change.defaultPrevented).toBe(true);
    expect(zoom.scale).toBeCloseTo(2, 6);
    const ox = (zoom.origin.x / 100) * SURFACE.w;
    const oy = (zoom.origin.y / 100) * SURFACE.h;
    expect(zoom.offset.x + ox + zoom.scale * (150 - ox)).toBeCloseTo(150, 6);
    expect(zoom.offset.y + oy + zoom.scale * (300 - oy)).toBeCloseTo(300, 6);
    gesture(el, "gestureend", { scale: 2, clientX: 150, clientY: 350 });
    expect(zoom.scale).toBeCloseTo(2, 6);
    // A gesture is not a touch pinch: the drag layer is not told to stand down.
    expect(isPinching()).toBe(false);
  });

  it("a gesture backed by a touch pinch is only cancelled; the touch path owns it", () => {
    touch(el, "touchstart", [
      { clientX: 150, clientY: 350 },
      { clientX: 250, clientY: 350 },
    ]);
    const g = gesture(el, "gesturechange", {
      scale: 3,
      clientX: 200,
      clientY: 350,
    });
    expect(g.defaultPrevented).toBe(true);
    expect(zoom).toBe(ZOOM_IDENTITY);
  });

  it("leaves gesture events alone when handleGestures is off (useWheelZoom owns them)", () => {
    detach();
    attach(false);
    const g = gesture(el, "gesturestart", { scale: 1, clientX: 0, clientY: 0 });
    expect(g.defaultPrevented).toBe(false);
  });

  it("detach removes every listener", () => {
    detach(); // (a second detach in afterEach is a harmless no-op)
    const start = touch(el, "touchstart", [
      { clientX: 100, clientY: 350 },
      { clientX: 200, clientY: 350 },
    ]);
    expect(start.defaultPrevented).toBe(false);
    touch(el, "touchmove", [
      { clientX: 50, clientY: 350 },
      { clientX: 250, clientY: 350 },
    ]);
    expect(zoom).toBe(ZOOM_IDENTITY);
  });
});
