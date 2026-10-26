// @vitest-environment jsdom
/**
 * `safeAreaCollisionPadding` keeps floating content (menus, popovers,
 * selects) out of the safe-area insets and, when a fixed bottom bar marked
 * `data-bottom-inset` is on screen, above that bar too — a menu that opened
 * over the phone tab bar had its last items visible but not tappable.
 */
import { afterEach, describe, expect, it } from "vitest";

import {
  EDGE_GUTTER,
  safeAreaCollisionPadding,
} from "@/components/folio/collision-padding";

afterEach(() => {
  document.body.innerHTML = "";
  document.documentElement.style.cssText = "";
});

function pad() {
  const p = safeAreaCollisionPadding();
  if (typeof p === "number") throw new Error("expected per-side padding");
  return p as Record<string, number>;
}

describe("safeAreaCollisionPadding", () => {
  it("is the gutter on every side when there are no insets and no bar", () => {
    expect(pad()).toEqual({
      top: EDGE_GUTTER,
      right: EDGE_GUTTER,
      bottom: EDGE_GUTTER,
      left: EDGE_GUTTER,
    });
  });

  it("adds the safe-area insets", () => {
    document.documentElement.style.setProperty("--safe-top", "47px");
    document.documentElement.style.setProperty("--safe-bottom", "34px");
    const p = pad();
    expect(p.top).toBe(47 + EDGE_GUTTER);
    expect(p.bottom).toBe(34 + EDGE_GUTTER);
  });

  it("keeps clear of a visible fixed bottom bar", () => {
    document.documentElement.style.setProperty("--safe-bottom", "34px");
    const bar = document.createElement("nav");
    bar.setAttribute("data-bottom-inset", "");
    document.body.appendChild(bar);
    // jsdom has no layout: give the bar a box at the bottom of an 800px
    // viewport, 90px tall (56px bar + 34px safe inset, as the tab bar is).
    Object.defineProperty(window, "innerHeight", {
      value: 800,
      configurable: true,
    });
    bar.getBoundingClientRect = () =>
      ({
        top: 710,
        bottom: 800,
        height: 90,
        left: 0,
        right: 390,
        width: 390,
        x: 0,
        y: 710,
        toJSON() {},
      }) as DOMRect;
    expect(pad().bottom).toBe(90 + EDGE_GUTTER);
  });

  it("ignores a bar that is hidden or off the bottom edge", () => {
    const bar = document.createElement("nav");
    bar.setAttribute("data-bottom-inset", "");
    bar.style.display = "none";
    document.body.appendChild(bar);
    Object.defineProperty(window, "innerHeight", {
      value: 800,
      configurable: true,
    });
    bar.getBoundingClientRect = () =>
      ({
        top: 710,
        bottom: 800,
        height: 90,
        left: 0,
        right: 390,
        width: 390,
        x: 0,
        y: 710,
        toJSON() {},
      }) as DOMRect;
    expect(pad().bottom).toBe(EDGE_GUTTER);
    bar.style.display = "";
    // Scrolled out of the viewport entirely: nothing to clear.
    bar.getBoundingClientRect = () =>
      ({
        top: 900,
        bottom: 990,
        height: 90,
        left: 0,
        right: 390,
        width: 390,
        x: 0,
        y: 900,
        toJSON() {},
      }) as DOMRect;
    expect(pad().bottom).toBe(EDGE_GUTTER);
  });
});
