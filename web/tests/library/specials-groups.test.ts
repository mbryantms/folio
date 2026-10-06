/**
 * Specials & Extras sections (series Issues tab): grouping by
 * `special_type` into a fixed section order with per-group number sort,
 * and the special-aware card number label.
 */
import { describe, expect, it } from "vitest";

import { groupSpecials } from "@/app/[locale]/(library)/series/[slug]/SpecialsExtrasSection";
import { issueNumberLabel } from "@/components/library/IssueCard";
import type { IssueSummaryView } from "@/lib/api/types";

function issue(overrides: Partial<IssueSummaryView>): IssueSummaryView {
  return {
    id: "i",
    slug: "i-slug",
    series_id: "s1",
    series_slug: "series",
    title: null,
    number: null,
    sort_number: null,
    year: null,
    page_count: null,
    state: "active",
    cover_url: null,
    special_type: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  } as IssueSummaryView;
}

describe("groupSpecials", () => {
  it("orders sections Annuals → Specials → One-shots → Collected, then others", () => {
    const groups = groupSpecials([
      issue({ id: "t", special_type: "TPB" }),
      issue({ id: "x", special_type: "Zine" }),
      issue({ id: "o", special_type: "OneShot" }),
      issue({ id: "a2", special_type: "Annual", number: "2", sort_number: 2 }),
      issue({ id: "s", special_type: "Special" }),
      issue({ id: "a1", special_type: "Annual", number: "1", sort_number: 1 }),
      issue({ id: "main", special_type: null }),
    ]);
    expect(groups.map((g) => g.label)).toEqual([
      "Annuals",
      "Specials",
      "One-shots",
      "Collected editions",
      "Zine",
    ]);
    expect(groups[0].items.map((i) => i.id)).toEqual(["a1", "a2"]);
    expect(groups.flatMap((g) => g.items).some((i) => i.id === "main")).toBe(
      false,
    );
  });

  it("drops empty groups", () => {
    expect(groupSpecials([])).toEqual([]);
    expect(groupSpecials([issue({ special_type: null })])).toEqual([]);
  });
});

describe("issueNumberLabel", () => {
  it("names the kind on specials and keeps plain numbers for the run", () => {
    expect(issueNumberLabel({ number: "12" })).toBe("#12");
    expect(issueNumberLabel({ number: null })).toBe("—");
    expect(issueNumberLabel({ number: "1", special_type: "Annual" })).toBe(
      "Annual #1",
    );
    expect(issueNumberLabel({ number: null, special_type: "OneShot" })).toBe(
      "One-shot",
    );
    expect(issueNumberLabel({ number: "3", special_type: "TPB" })).toBe(
      "Collected edition #3",
    );
    expect(issueNumberLabel({ number: "1", special_type: "Zine" })).toBe(
      "Zine #1",
    );
  });
});
