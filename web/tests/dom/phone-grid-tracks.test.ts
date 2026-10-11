/**
 * A card grid that declares its column count only from a breakpoint up
 * (`grid gap-3 sm:grid-cols-2 …`) leaves the phone-width track as the
 * implicit `auto`, whose minimum is the widest nowrap child — a long
 * reading-list name pushed the whole card past an iPhone's viewport
 * (Settings → Views → Reading lists). `grid-cols-1` pins the base track
 * to `minmax(0, 1fr)` like the breakpoint tracks already are. This test
 * keeps the pattern from coming back.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const ROOTS = ["components", "app"].map((d) => join(process.cwd(), d));
const PATTERN = /className="grid gap-[\d.]+ (?:sm|md|lg|xl):grid-cols-/;

function tsxFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...tsxFiles(p));
    else if (p.endsWith(".tsx")) out.push(p);
  }
  return out;
}

describe("card grids declare a phone-width track", () => {
  it("has no breakpoint-only grid-cols without a grid-cols-1 base", () => {
    const offenders = ROOTS.flatMap(tsxFiles).filter((f) =>
      PATTERN.test(readFileSync(f, "utf8")),
    );
    expect(offenders).toEqual([]);
  });
});
