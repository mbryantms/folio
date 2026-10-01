/**
 * Pure decision logic behind the reader's swipe-to-turn gesture.
 *
 * `swipeAction` pins the threshold + reading-direction mapping shared
 * by the primary gesture binding and the resume-wedge pointer
 * fallback, so the two paths can't drift apart.
 *
 * `isPinchZoomed` pins the corroborated pinch-zoom guard: iOS
 * standalone PWAs can resume from the background with a stale
 * `visualViewport.scale > 1`, and the old raw-scale check then
 * silently ate every swipe (drag completed, lift declined to act,
 * taps kept working) until the app was force-quit. The guard now
 * requires the layout-vs-visual width ratio to back the scale up.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  EDGE_BACK_INSET_PX,
  inEdgeBackInset,
  isEdgeTap,
  isIosStandalone,
  isPinchZoomed,
  swipeAction,
} from "@/lib/reader/use-swipe";

describe("swipeAction", () => {
  it("ignores drags under the 30px threshold", () => {
    expect(swipeAction(0, "ltr")).toBeNull();
    expect(swipeAction(29, "ltr")).toBeNull();
    expect(swipeAction(-29, "ltr")).toBeNull();
    expect(swipeAction(29, "rtl")).toBeNull();
  });

  it("maps swipe-left to next and swipe-right to prev in LTR", () => {
    expect(swipeAction(-30, "ltr")).toBe("next");
    expect(swipeAction(-200, "ltr")).toBe("next");
    expect(swipeAction(30, "ltr")).toBe("prev");
    expect(swipeAction(200, "ltr")).toBe("prev");
  });

  it("inverts the mapping in RTL", () => {
    expect(swipeAction(30, "rtl")).toBe("next");
    expect(swipeAction(-30, "rtl")).toBe("prev");
  });
});

describe("isPinchZoomed", () => {
  it("is false at rest (scale 1, matching widths)", () => {
    expect(isPinchZoomed(1, 390, 390)).toBe(false);
  });

  it("tolerates jittery near-1 scale readings", () => {
    expect(isPinchZoomed(1.04, 390, 390)).toBe(false);
  });

  it("is true when genuinely pinch-zoomed (widths corroborate)", () => {
    // scale 2 => visual viewport is half the layout viewport wide.
    expect(isPinchZoomed(2, 195, 390)).toBe(true);
    expect(isPinchZoomed(1.2, 325, 390)).toBe(true);
  });

  it("treats a scale the widths don't back as stale (the PWA resume bug)", () => {
    // visualViewport claims zoomed, but visual and layout widths are
    // equal — the reading is stale state from a background/resume
    // cycle, not a real pinch. The swipe must not be eaten.
    expect(isPinchZoomed(1.2, 390, 390)).toBe(false);
    expect(isPinchZoomed(2, 390, 390)).toBe(false);
  });

  it("falls back to trusting the scale when widths are unusable", () => {
    expect(isPinchZoomed(2, 0, 390)).toBe(true);
    expect(isPinchZoomed(2, 195, 0)).toBe(true);
  });
});

// ---- WP-4.2 / audit UX-10: iOS standalone edge-back guard ----

describe("inEdgeBackInset", () => {
  it("claims touches in the left-edge strip only", () => {
    expect(inEdgeBackInset(0)).toBe(true);
    expect(inEdgeBackInset(EDGE_BACK_INSET_PX - 1)).toBe(true);
    expect(inEdgeBackInset(EDGE_BACK_INSET_PX)).toBe(false);
    expect(inEdgeBackInset(400)).toBe(false);
  });

  it("ignores negative coordinates (touches outside the viewport)", () => {
    expect(inEdgeBackInset(-5)).toBe(false);
  });
});

describe("isEdgeTap", () => {
  const start = { x: 10, y: 300, t: 1000 };

  it("recognizes a short, still touch as a tap", () => {
    expect(isEdgeTap(start, { x: 14, y: 303, t: 1150 })).toBe(true);
  });

  it("rejects a swipe (too much travel)", () => {
    expect(isEdgeTap(start, { x: 120, y: 300, t: 1150 })).toBe(false);
  });

  it("rejects a long press", () => {
    expect(isEdgeTap(start, { x: 10, y: 300, t: 2000 })).toBe(false);
  });
});

describe("isIosStandalone", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("is false with no window (SSR)", () => {
    expect(isIosStandalone()).toBe(false);
  });

  it("is true only when navigator.standalone === true (Apple WebKit)", () => {
    vi.stubGlobal("window", { navigator: { standalone: true } });
    expect(isIosStandalone()).toBe(true);
    vi.stubGlobal("window", { navigator: { standalone: false } });
    expect(isIosStandalone()).toBe(false);
    // Android / desktop standalone PWAs have no `standalone` property.
    vi.stubGlobal("window", { navigator: {} });
    expect(isIosStandalone()).toBe(false);
  });
});
