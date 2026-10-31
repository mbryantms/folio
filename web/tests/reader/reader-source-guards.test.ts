/**
 * Source guards for the reader route (audit 2026-11, N-R5 / N-R12 / N-R4):
 *
 *  - no label below `text-xs` (12px) in reader chrome — `text-[10px]`,
 *    `text-[11px]` and the like (closes AC-4);
 *  - `Reader.tsx` carries no `react-hooks/exhaustive-deps` suppressions
 *    (the four that tolerated a render-scoped `RECENTER` literal are gone);
 *  - page images never hard-code a `Page N` alt: the label comes from
 *    `usePageAltText`, which quotes the page-text panel's first OCR line
 *    once it has one (behaviour covered in tests/dom/reader-a11y).
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const READER_DIR = join(
  process.cwd(),
  "app",
  "[locale]",
  "read",
  "[seriesSlug]",
  "[issueSlug]",
);

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) return walk(full);
    return /\.(tsx?|css)$/.test(name) ? [full] : [];
  });
}

describe("reader route source guards", () => {
  const files = walk(READER_DIR);

  it("finds the reader route", () => {
    expect(files.some((f) => f.endsWith("Reader.tsx"))).toBe(true);
  });

  it("has no label smaller than text-xs (AC-4)", () => {
    const offenders = files.flatMap((f) => {
      const src = readFileSync(f, "utf8");
      return src
        .split("\n")
        .map((line, i) =>
          /text-\[(?:[1-9]|1[01])px\]/.test(line) ? `${f}:${i + 1}` : null,
        )
        .filter((x): x is string => x !== null);
    });
    expect(offenders).toEqual([]);
  });

  it("Reader.tsx has no exhaustive-deps suppressions (N-R12)", () => {
    const src = readFileSync(join(READER_DIR, "Reader.tsx"), "utf8");
    expect(src).not.toMatch(/eslint-disable[^\n]*react-hooks\/exhaustive-deps/);
  });

  it("page images do not hard-code a plain 'Page N' alt (N-R4)", () => {
    const src = readFileSync(join(READER_DIR, "Reader.tsx"), "utf8");
    expect(src).not.toMatch(/alt=\{`Page \$\{/);
  });
});
