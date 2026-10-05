import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * iOS / iPadOS 26 home-screen apps blur ~40pt over the content at a screen
 * edge unless a fixed / sticky full-width bar there paints one solid
 * colour (see the `standalone` variant in styles/globals.css). Every
 * edge-docked bar that is translucent in a browser tab must therefore go
 * opaque in the installed app.
 */
const root = join(__dirname, "../..");
const read = (p: string) => readFileSync(join(root, p), "utf8");

const EDGE_BAR_FILES = [
  "components/library/MainShell.tsx",
  "components/admin/AdminShell.tsx",
  "components/library/BottomTabBar.tsx",
  "app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderChrome.tsx",
  "app/[locale]/read/[seriesSlug]/[issueSlug]/loading.tsx",
];

/** Class lists of bars docked to the top or bottom edge with a frosted
 *  (translucent + backdrop-blur) background. */
function edgeBars(src: string): string[] {
  return [...src.matchAll(/className="([^"]+)"/g)]
    .map((m) => m[1] ?? "")
    .filter((cls) => {
      const t = cls.split(/\s+/);
      return (
        (t.includes("fixed") || t.includes("sticky")) &&
        (t.includes("top-0") || t.includes("bottom-0")) &&
        t.includes("backdrop-blur")
      );
    });
}

describe("iOS 26 scroll edge effect", () => {
  it("declares the standalone variant", () => {
    expect(read("styles/globals.css")).toMatch(
      /@custom-variant standalone \(@media \(display-mode: standalone\)\);/,
    );
  });

  it.each(EDGE_BAR_FILES)("%s: edge bars are opaque when installed", (file) => {
    const bars = edgeBars(read(file));
    expect(bars.length, "no edge-docked frosted bar found").toBeGreaterThan(0);
    for (const cls of bars) {
      const t = cls.split(/\s+/);
      expect(t, cls).toContain("standalone:backdrop-blur-none");
      expect(
        t.some((x) => /^standalone:bg-[a-z0-9-]+$/.test(x)),
        `${cls} needs an opaque standalone:bg-*`,
      ).toBe(true);
    }
  });

  it("the reader's status-bar backing is solid, not a fade", () => {
    const src = read("app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx");
    expect(src).toContain("h-(--reader-top) bg-black");
    expect(src).not.toMatch(/h-\(--(safe|reader)-top\) bg-gradient/);
  });

  it("the installed reader's top bar is thick enough for WebKit to extend", () => {
    // WebKit skips fixed edge bars 10px or thinner (`thinBorderWidth`), so
    // a 0-height backing (--safe-top pinned to 0) let the blur through.
    const css = read("styles/globals.css");
    expect(css).toMatch(
      /@media \(display-mode: standalone\)\s*\{\s*:root\s*\{\s*--reader-top: max\(var\(--safe-top\), 12px\);/,
    );
    const src = read("app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx");
    expect(src).not.toMatch(/pt-\(--safe-top\) pb-\(--safe-bottom\)/);
  });

  it("the root layout keeps ONE persistent top-edge element above every header", () => {
    // iPadOS 27 latches into content blur the first time the element it
    // samples at the top edge CHANGES (a header unmounting on a layout
    // switch, the reader chrome taking over, an overlay opening). Measured
    // with /pwa-lab Q: shrinking the same header kept the strip clean;
    // swapping in any other bar, at any height, fixed or sticky, latched
    // it. So #top-edge is always the sampled element: first in <body>,
    // sticky, above everything, 12px (WebKit ignores bars ≤ 10px), in the
    // page colour; the reader turns it black.
    const src = read("app/layout.tsx");
    expect(src).toMatch(
      /id="top-edge"[\s\S]{0,200}className="pointer-events-none sticky top-0 z-\[9999\] -mb-3 h-3 bg-\[var\(--top-edge-color,hsl\(var\(--background\)\)\)\]"/,
    );
    expect(src.indexOf('id="top-edge"')).toBeLessThan(
      src.indexOf("<ThemeProvider"),
    );
    expect(read("styles/globals.css")).toMatch(
      /html:has\(\[data-surface="reader"\]\)\s*\{\s*--top-edge-color: #000;/,
    );
    expect(
      read("app/[locale]/read/[seriesSlug]/[issueSlug]/Reader.tsx"),
    ).toContain('data-surface="reader"');
  });
});
