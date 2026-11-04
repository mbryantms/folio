/** Pure logic behind the folio-flair shell motion: the bottom-bar
 *  hide-on-scroll decision, the sliding nav pill geometry, the page
 *  cross-fade key, and the palette's context helpers. */
import { describe, expect, it } from "vitest";

import {
  initialHideOnScrollState,
  nextHideOnScroll,
  type HideOnScrollState,
} from "@/lib/ui/hide-on-scroll";
import { measurePill } from "@/lib/ui/nav-pill";
import { routeFadeKey } from "@/lib/ui/route-fade";
import {
  nextToggleTheme,
  unavailableActionIds,
} from "@/lib/search/use-search-actions";

const MAX = 5000;
function scrollThrough(
  ys: number[],
  opts: { blocked?: boolean; start?: HideOnScrollState } = {},
): HideOnScrollState {
  let s = opts.start ?? initialHideOnScrollState(0);
  for (const y of ys) {
    s = nextHideOnScroll(s, { y, maxY: MAX, blocked: !!opts.blocked });
  }
  return s;
}

describe("nextHideOnScroll", () => {
  it("hides after sustained downward travel past the top zone", () => {
    const start = initialHideOnScrollState(100);
    expect(scrollThrough([110], { start }).hidden).toBe(false);
    expect(scrollThrough([110, 130], { start }).hidden).toBe(true);
  });

  it("returns after sustained upward travel", () => {
    const down = scrollThrough([100, 200, 300]);
    expect(down.hidden).toBe(true);
    // A small wobble up doesn't bring it back…
    expect(scrollThrough([290], { start: down }).hidden).toBe(true);
    // …but a real scroll up does.
    expect(scrollThrough([290, 270], { start: down }).hidden).toBe(false);
  });

  it("a direction reversal restarts the accumulator", () => {
    const s = scrollThrough([100, 200, 300, 290]);
    expect(s.acc).toBe(-10);
    // Going down again needs the full threshold again.
    expect(scrollThrough([300], { start: s }).acc).toBe(10);
  });

  it("always shows near the top, including iOS rubber-band negatives", () => {
    const down = scrollThrough([100, 200, 300]);
    expect(scrollThrough([30], { start: down }).hidden).toBe(false);
    expect(scrollThrough([-20], { start: down }).hidden).toBe(false);
  });

  it("shows at the page end", () => {
    const down = scrollThrough([100, 200, 300, 4000]);
    expect(down.hidden).toBe(true);
    expect(scrollThrough([MAX - 10], { start: down }).hidden).toBe(false);
  });

  it("never hides while an overlay is open, and resets", () => {
    const s = scrollThrough([100, 200, 300, 400], { blocked: true });
    expect(s).toEqual({ hidden: false, acc: 0, lastY: 400 });
  });

  it("a page shorter than the top zone never treats the top as the end", () => {
    const s = nextHideOnScroll(initialHideOnScrollState(0), {
      y: 10,
      maxY: 20,
      blocked: false,
    });
    expect(s.hidden).toBe(false);
  });
});

describe("measurePill", () => {
  const nav = { left: 100, top: 50, width: 240, height: 600 };
  it("positions in the container's coordinate space", () => {
    expect(
      measurePill(nav, { left: 108, top: 90, width: 224, height: 32 }),
    ).toEqual({ x: 8, y: 40, width: 224, height: 32 });
  });
  it("applies the inset on each side", () => {
    expect(
      measurePill(
        { left: 0, top: 800, width: 390, height: 56 },
        { left: 78, top: 800, width: 78, height: 56 },
        { x: 10, y: 5 },
      ),
    ).toEqual({ x: 88, y: 5, width: 58, height: 46 });
  });
  it("returns null for an item that isn't laid out", () => {
    expect(
      measurePill(nav, { left: 0, top: 0, width: 0, height: 0 }),
    ).toBeNull();
    expect(
      measurePill(nav, { left: 0, top: 0, width: 10, height: 8 }, { y: 5 }),
    ).toBeNull();
  });
});

describe("routeFadeKey", () => {
  it("splits / by library", () => {
    expect(routeFadeKey("/", null)).toBe("/");
    expect(routeFadeKey("/", "all")).toBe("/?library=all");
    expect(routeFadeKey("/", "abc")).not.toBe(routeFadeKey("/", "all"));
  });
  it("keeps the key within a section (no fade between details)", () => {
    expect(routeFadeKey("/series/a", null)).toBe(
      routeFadeKey("/series/b", null),
    );
    expect(routeFadeKey("/series/a/issues/1", null)).toBe("/series");
    expect(routeFadeKey("/collections", null)).toBe(
      routeFadeKey("/collections/x", null),
    );
  });
  it("keys settings, admin, views and pages on two segments", () => {
    expect(routeFadeKey("/settings/account", null)).not.toBe(
      routeFadeKey("/settings/theme", null),
    );
    expect(routeFadeKey("/admin/users", null)).toBe("/admin/users");
    expect(routeFadeKey("/admin", null)).toBe("/admin/");
    expect(routeFadeKey("/views/want-to-read", null)).not.toBe(
      routeFadeKey("/views/other", null),
    );
  });
  it("library-to-series changes the key (fade alongside the cover morph)", () => {
    expect(routeFadeKey("/", "all")).not.toBe(routeFadeKey("/series/x", null));
  });
});

describe("palette context helpers", () => {
  it("nextToggleTheme flips light ↔ dark-family", () => {
    expect(nextToggleTheme("light")).toBe("dark");
    expect(nextToggleTheme("dark")).toBe("light");
    expect(nextToggleTheme("amber")).toBe("light");
    expect(nextToggleTheme(undefined)).toBe("light");
  });
  it("hides the sidebar toggle in the reader and continue without progress", () => {
    expect(
      unavailableActionIds({ pathname: "/read/a/1", hasContinue: true }),
    ).toEqual(new Set(["cmd-toggle-sidebar"]));
    expect(
      unavailableActionIds({ pathname: "/collections", hasContinue: false }),
    ).toEqual(new Set(["cmd-continue-reading"]));
    expect(
      unavailableActionIds({ pathname: "/reading-x", hasContinue: true }).size,
    ).toBe(0);
  });
});
