/**
 * `components/ui/` is vendored pristine from shadcn, and the weekly
 * `shadcn-refresh` PR overwrites any local edit there (docs/dev/
 * shadcn-theming-audit.md, PR 4). Folio's look for a vendored primitive
 * therefore lives in `globals.css` on its `data-slot` hook. Two folio-flair
 * treatments used to be edits inside the files and were lost by the
 * 2026-11-09 refresh: the shared skeleton sweep and the badge's
 * `rounded-md` corners. These keep them on the hooks.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(join(process.cwd(), "styles/globals.css"), "utf8");
const vendored = (file: string) =>
  readFileSync(join(process.cwd(), "components/ui", file), "utf8");

describe("vendored primitives keep Folio's look through data-slot", () => {
  it("the vendored <Skeleton> gets the shared sweep, frozen under reduced motion", () => {
    expect(vendored("skeleton.tsx")).toContain('data-slot="skeleton"');
    expect(css).toMatch(
      /\.skeleton-shimmer,\s*\[data-slot="skeleton"\]\s*\{[^}]*animation:\s*skeleton-sweep/,
    );
    // The OS preference and the account's reduced-motion setting.
    expect(css).toMatch(
      /prefers-reduced-motion: reduce\)\s*\{\s*\.skeleton-shimmer,\s*\[data-slot="skeleton"\]\s*\{\s*animation:\s*none/,
    );
    expect(css).toMatch(
      /\[data-motion="reduced"\] \[data-slot="skeleton"\]\s*\{\s*animation:\s*none/,
    );
  });

  it("badges keep rounded-md corners instead of upstream's pill", () => {
    expect(vendored("badge.tsx")).toContain('data-slot="badge"');
    expect(css).toMatch(
      /\[data-slot="badge"\]\[data-slot="badge"\]\s*\{\s*border-radius:\s*var\(--radius-md\)/,
    );
  });
});
