import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { themedViewport } from "@/lib/viewport";

/**
 * An installed iPadOS app latches the first runtime change to the
 * theme-color / color-scheme metas into a blurred status-bar strip that
 * persists until force-quit. The reader therefore must not declare its
 * own viewport: every route shares the root one (see lib/viewport.ts).
 */
describe("reader viewport", () => {
  it("the reader route inherits the root viewport (no generateViewport)", () => {
    const src = readFileSync(
      join(
        __dirname,
        "../../app/[locale]/read/[seriesSlug]/[issueSlug]/page.tsx",
      ),
      "utf8",
    );
    // Only a real export counts — the explanatory comment names the function.
    expect(src).not.toMatch(
      /export (async )?function generateViewport|export const viewport|readerViewport/,
    );
  });

  it("the root viewport declares a media-query theme-color pair for system", () => {
    const v = themedViewport("system");
    expect(v.colorScheme).toBe("dark light");
    expect(Array.isArray(v.themeColor)).toBe(true);
  });
});
