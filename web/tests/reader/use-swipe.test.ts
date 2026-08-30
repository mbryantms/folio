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
import { describe, expect, it } from "vitest";
import { isPinchZoomed, swipeAction } from "@/lib/reader/use-swipe";

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
