import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { themeHeadMeta } from "@/lib/viewport";

const root = join(__dirname, "../..");
const read = (p: string) => readFileSync(join(root, p), "utf8");

/**
 * Next re-renders every metadata-API tag on each client navigation
 * (remove + re-insert). An installed iPadOS app latches that into a
 * permanently blurred status-bar strip, so the Apple PWA tags, theme-color
 * and color-scheme must be static children of the root layout's <head>
 * and must never come from `metadata` / `generateViewport` — on any route.
 */
describe("navigation-stable head tags", () => {
  const layout = read("app/layout.tsx");

  it("root layout renders the Apple + colour metas statically", () => {
    expect(layout).toMatch(/<head>/);
    for (const name of [
      "apple-mobile-web-app-capable",
      "mobile-web-app-capable",
      "apple-mobile-web-app-title",
      "apple-mobile-web-app-status-bar-style",
      "color-scheme",
      "theme-color",
    ]) {
      expect(layout, name).toContain(`name="${name}"`);
    }
  });

  it("no route declares them through the metadata / viewport API", () => {
    expect(layout).not.toMatch(/appleWebApp|themeColor:|colorScheme:/);
    expect(read("lib/viewport.ts")).not.toMatch(
      /export (const|function) (themedViewport|readerViewport)/,
    );
    expect(
      read("app/[locale]/read/[seriesSlug]/[issueSlug]/page.tsx"),
    ).not.toMatch(
      /export (async )?function generateViewport|export const viewport/,
    );
  });

  it("themeHeadMeta follows the cookie theme", () => {
    expect(themeHeadMeta("dark")).toEqual({
      colorScheme: "dark",
      themeColor: [{ color: "#0c0e13" }],
    });
    expect(themeHeadMeta("system").colorScheme).toBe("dark light");
    expect(themeHeadMeta("system").themeColor).toHaveLength(2);
  });
});
