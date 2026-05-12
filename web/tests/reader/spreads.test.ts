/**
 * Spread-group derivation matrix. Each test pins a specific arrangement
 * of cover/pair/spread so a future tweak to `computeSpreadGroups` can't
 * silently shift the visual pairing for users mid-issue.
 */
import { describe, expect, it } from "vitest";
import type { PageInfo } from "@/lib/api/types";
import {
  computeSpreadGroups,
  firstPageOfGroup,
  groupIndexForPage,
  visiblePagesAt,
} from "@/lib/reader/spreads";

const single = (image: number): PageInfo => ({ image });
const spread = (image: number): PageInfo => ({ image, double_page: true });

describe("computeSpreadGroups", () => {
  it("returns empty for an empty issue", () => {
    expect(computeSpreadGroups([])).toEqual([]);
  });

  it("renders a single-page issue as one solo group", () => {
    expect(computeSpreadGroups([single(0)])).toEqual([[0]]);
  });

  it("treats the cover as solo by default and pairs from index 1", () => {
    const groups = computeSpreadGroups([
      single(0),
      single(1),
      single(2),
      single(3),
      single(4),
    ]);
    expect(groups).toEqual([[0], [1, 2], [3, 4]]);
  });

  it("pairs from index 0 when coverSolo is false", () => {
    const groups = computeSpreadGroups(
      [single(0), single(1), single(2), single(3)],
      { coverSolo: false },
    );
    expect(groups).toEqual([
      [0, 1],
      [2, 3],
    ]);
  });

  it("renders a mid-archive double as solo and resumes pairing after", () => {
    // Pages: [cover, p1, p2, SPREAD@3, p4, p5]
    const groups = computeSpreadGroups([
      single(0),
      single(1),
      single(2),
      spread(3),
      single(4),
      single(5),
    ]);
    expect(groups).toEqual([[0], [1, 2], [3], [4, 5]]);
  });

  it("does not pair a single with a *following* spread", () => {
    // Pages: [cover, p1, SPREAD@2, p3] — p1 must NOT pair with the spread.
    const groups = computeSpreadGroups([
      single(0),
      single(1),
      spread(2),
      single(3),
    ]);
    expect(groups).toEqual([[0], [1], [2], [3]]);
  });

  it("emits two solo groups for adjacent spreads", () => {
    const groups = computeSpreadGroups([
      single(0),
      spread(1),
      spread(2),
      single(3),
      single(4),
    ]);
    expect(groups).toEqual([[0], [1], [2], [3, 4]]);
  });

  it("renders an odd-page-count tail as a final solo", () => {
    const groups = computeSpreadGroups([
      single(0),
      single(1),
      single(2),
      single(3), // unpaired tail
    ]);
    expect(groups).toEqual([[0], [1, 2], [3]]);
  });

  it("renders a spread at the final index as solo", () => {
    const groups = computeSpreadGroups([
      single(0),
      single(1),
      single(2),
      spread(3),
    ]);
    expect(groups).toEqual([[0], [1, 2], [3]]);
  });

  it("matches the Geiger 004 fixture: spreads at 9, 26, 27 (out of 32)", () => {
    const pages: PageInfo[] = Array.from({ length: 32 }, (_, i) =>
      i === 9 || i === 26 || i === 27 ? spread(i) : single(i),
    );
    const groups = computeSpreadGroups(pages);
    // Expect: cover solo, 14 pairs covering 1-8 + 10-25, three solo groups
    // [9], [26], [27], then a final pair [28,29], pair [30,31].
    expect(groups[0]).toEqual([0]);
    expect(groups).toContainEqual([9]);
    expect(groups).toContainEqual([26]);
    expect(groups).toContainEqual([27]);
    // Adjacent-to-spread pages must be solo or paired with the prior page,
    // never paired into the spread.
    expect(groups.find((g) => g.includes(8) && g.includes(9))).toBeUndefined();
    expect(
      groups.find((g) => g.includes(25) && g.includes(26)),
    ).toBeUndefined();
    // Total visited indices = 32.
    const flat = groups.flat();
    expect(new Set(flat).size).toBe(32);
    expect(flat.length).toBe(32);
  });
});

describe("groupIndexForPage / firstPageOfGroup / visiblePagesAt", () => {
  const pages: PageInfo[] = [
    single(0),
    single(1),
    single(2),
    spread(3),
    single(4),
    single(5),
  ];
  const groups = computeSpreadGroups(pages);
  // groups: [[0], [1,2], [3], [4,5]]

  it("maps a page to its enclosing group", () => {
    expect(groupIndexForPage(groups, 0)).toBe(0);
    expect(groupIndexForPage(groups, 1)).toBe(1);
    expect(groupIndexForPage(groups, 2)).toBe(1);
    expect(groupIndexForPage(groups, 3)).toBe(2);
    expect(groupIndexForPage(groups, 5)).toBe(3);
  });

  it("returns the anchor page of a group", () => {
    expect(firstPageOfGroup(groups, 0)).toBe(0);
    expect(firstPageOfGroup(groups, 1)).toBe(1);
    expect(firstPageOfGroup(groups, 2)).toBe(3);
    expect(firstPageOfGroup(groups, 3)).toBe(4);
  });

  it("clamps out-of-range group indices", () => {
    expect(firstPageOfGroup(groups, -5)).toBe(0);
    expect(firstPageOfGroup(groups, 999)).toBe(4);
    expect(visiblePagesAt(groups, 999)).toEqual([4, 5]);
  });

  it("returns the visible-pages tuple for the group", () => {
    expect(visiblePagesAt(groups, 0)).toEqual([0]);
    expect(visiblePagesAt(groups, 1)).toEqual([1, 2]);
    expect(visiblePagesAt(groups, 2)).toEqual([3]);
  });
});
