/**
 * The universal thin-scrollbar rule must live in `@layer base`. Unlayered
 * author CSS beats every cascade layer, so when it sat outside a layer it
 * overrode the `[scrollbar-width:none]` utility on tab strips and rails
 * and drew a scrollbar under the CBL sheet's tabs (2026-10-13).
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

describe("global scrollbar rule", () => {
  it("is declared inside @layer base so element utilities win", () => {
    const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");
    const at = css.indexOf("scrollbar-width: thin");
    expect(at).toBeGreaterThan(0);
    // The nearest enclosing `@layer` before the rule must be `base`, and
    // its block must still be open where the rule sits.
    const before = css.slice(0, at);
    const layerAt = before.lastIndexOf("@layer base");
    expect(layerAt).toBeGreaterThan(0);
    const between = before.slice(layerAt);
    const opens = (between.match(/\{/g) ?? []).length;
    const closes = (between.match(/\}/g) ?? []).length;
    expect(opens).toBeGreaterThan(closes);
  });
});
