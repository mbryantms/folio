/**
 * WP-4.3 — manual spread controls. The "done when" contract: overrides
 * win over both `DoublePage` metadata and aspect-ratio detection, and a
 * one-page shift fixes an offset scan.
 */
import { describe, expect, it } from "vitest";
import type { PageInfo } from "@/lib/api/types";
import {
  EMPTY_SPREAD_OVERRIDES,
  computeSpreadGroups,
  isEffectiveSpread,
  isEmptySpreadOverrides,
  nextPageSpreadMode,
  pageSpreadMode,
  withPageSpreadMode,
  type SpreadOverrides,
} from "@/lib/reader/spreads";

const single = (image: number): PageInfo => ({ image });
const flagged = (image: number): PageInfo => ({ image, double_page: true });
const landscape = (image: number): PageInfo => ({
  image,
  image_width: 2000,
  image_height: 1400,
});
const portrait = (image: number): PageInfo => ({
  image,
  image_width: 1000,
  image_height: 1500,
});

const ov = (o: Partial<SpreadOverrides>): SpreadOverrides => ({
  ...EMPTY_SPREAD_OVERRIDES,
  ...o,
});

describe("overrides win over DoublePage metadata", () => {
  const pages = [single(0), single(1), flagged(2), single(3), single(4)];

  it("baseline: the flagged page is solo", () => {
    expect(computeSpreadGroups(pages)).toEqual([[0], [1], [2], [3, 4]]);
  });

  it("force single: a flagged page pairs like an ordinary page", () => {
    expect(
      computeSpreadGroups(pages, { overrides: ov({ single_pages: [2] }) }),
    ).toEqual([[0], [1, 2], [3, 4]]);
  });

  it("force spread: an unflagged page renders solo", () => {
    expect(
      computeSpreadGroups([single(0), single(1), single(2), single(3)], {
        overrides: ov({ spread_pages: [1] }),
      }),
    ).toEqual([[0], [1], [2, 3]]);
  });
});

describe("overrides win over aspect-ratio detection", () => {
  it("force single on a landscape page lets it pair", () => {
    const pages = [portrait(0), portrait(1), landscape(2), portrait(3)];
    expect(computeSpreadGroups(pages)).toEqual([[0], [1], [2], [3]]);
    expect(
      computeSpreadGroups(pages, { overrides: ov({ single_pages: [2] }) }),
    ).toEqual([[0], [1, 2], [3]]);
  });

  it("force spread on a portrait page keeps it solo", () => {
    const pages = [portrait(0), portrait(1), portrait(2), portrait(3)];
    expect(
      computeSpreadGroups(pages, { overrides: ov({ spread_pages: [2] }) }),
    ).toEqual([[0], [1], [2], [3]]);
  });

  it("isEffectiveSpread mirrors the override precedence", () => {
    const pages = [landscape(0), portrait(1), flagged(2)];
    const o = ov({ spread_pages: [1], single_pages: [0, 2] });
    expect(isEffectiveSpread(pages, 0, o)).toBe(false);
    expect(isEffectiveSpread(pages, 1, o)).toBe(true);
    expect(isEffectiveSpread(pages, 2, o)).toBe(false);
    expect(isEffectiveSpread(pages, 0, null)).toBe(true);
    expect(isEffectiveSpread(pages, 2, null)).toBe(true);
  });
});

describe("shift pairing by one", () => {
  const six = [0, 1, 2, 3, 4, 5].map(single);

  it("with cover solo: the first pairable page goes solo", () => {
    expect(computeSpreadGroups(six)).toEqual([[0], [1, 2], [3, 4], [5]]);
    expect(
      computeSpreadGroups(six, { overrides: ov({ shift_pairing: true }) }),
    ).toEqual([[0], [1], [2, 3], [4, 5]]);
  });

  it("without cover solo: pairs start one page later", () => {
    expect(computeSpreadGroups(six, { coverSolo: false })).toEqual([
      [0, 1],
      [2, 3],
      [4, 5],
    ]);
    expect(
      computeSpreadGroups(six, {
        coverSolo: false,
        overrides: ov({ shift_pairing: true }),
      }),
    ).toEqual([[0], [1, 2], [3, 4], [5]]);
  });

  it("is not consumed by a page that was solo anyway", () => {
    // Page 1 is solo because page 2 is a spread; the shift applies to
    // the first real pair (3,4) instead.
    const pages = [single(0), single(1), flagged(2), single(3), single(4)];
    expect(
      computeSpreadGroups(pages, { overrides: ov({ shift_pairing: true }) }),
    ).toEqual([[0], [1], [2], [3], [4]]);
  });

  it("combines with a forced spread mid-issue", () => {
    expect(
      computeSpreadGroups(six, {
        overrides: ov({ shift_pairing: true, spread_pages: [3] }),
      }),
    ).toEqual([[0], [1], [2], [3], [4, 5]]);
  });

  it("empty overrides are a no-op", () => {
    expect(
      computeSpreadGroups(six, { overrides: EMPTY_SPREAD_OVERRIDES }),
    ).toEqual(computeSpreadGroups(six));
  });
});

describe("override editing helpers", () => {
  it("cycles auto → spread → single → auto", () => {
    expect(nextPageSpreadMode("auto")).toBe("spread");
    expect(nextPageSpreadMode("spread")).toBe("single");
    expect(nextPageSpreadMode("single")).toBe("auto");
  });

  it("sets a page's mode keeping lists sorted and disjoint", () => {
    let o = withPageSpreadMode(null, 7, "spread");
    o = withPageSpreadMode(o, 3, "spread");
    expect(o.spread_pages).toEqual([3, 7]);
    expect(pageSpreadMode(o, 7)).toBe("spread");

    o = withPageSpreadMode(o, 7, "single");
    expect(o.spread_pages).toEqual([3]);
    expect(o.single_pages).toEqual([7]);
    expect(pageSpreadMode(o, 7)).toBe("single");

    o = withPageSpreadMode(o, 7, "auto");
    expect(o.single_pages).toEqual([]);
    expect(pageSpreadMode(o, 7)).toBe("auto");
  });

  it("preserves the shift flag", () => {
    const o = withPageSpreadMode(ov({ shift_pairing: true }), 2, "spread");
    expect(o.shift_pairing).toBe(true);
  });

  it("detects empty override sets", () => {
    expect(isEmptySpreadOverrides(null)).toBe(true);
    expect(isEmptySpreadOverrides(EMPTY_SPREAD_OVERRIDES)).toBe(true);
    expect(isEmptySpreadOverrides(ov({ shift_pairing: true }))).toBe(false);
    expect(isEmptySpreadOverrides(ov({ single_pages: [1] }))).toBe(false);
  });
});
