/**
 * Specials & Extras section — M6 of `scanner-nested-folders-1.0`.
 *
 * Asserts the section's sort/split helpers and the rendered output:
 * empty input → section hidden; populated → annuals group above
 * specials, with a stable per-group tiebreaker.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

// Mock IssueCard with a minimal stub — these tests assert the
// section's layout and the sort/split helpers, not IssueCard
// internals. Avoids dragging in router / TanStack Query / progress
// mutations for an SSR string render.
vi.mock("@/components/library/IssueCard", () => ({
  IssueCard: ({ issue }: { issue: { id: string; title: string | null } }) =>
    createElement(
      "article",
      { "data-issue-id": issue.id },
      issue.title ?? issue.id,
    ),
  IssueCardSkeleton: () => createElement("div", { "data-skeleton": true }),
}));

import {
  SpecialsExtrasSection,
  sortSpecials,
  splitMainAndSpecials,
} from "@/app/[locale]/(library)/series/[slug]/SpecialsExtrasSection";
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
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

describe("splitMainAndSpecials", () => {
  it("partitions issues by special_type truthiness", () => {
    const items = [
      issue({ id: "1", special_type: null }),
      issue({ id: "2", special_type: "Annual" }),
      issue({ id: "3" }), // special_type missing — main run
      issue({ id: "4", special_type: "Special" }),
    ];
    const { mainRun, specials } = splitMainAndSpecials(items);
    expect(mainRun.map((i) => i.id)).toEqual(["1", "3"]);
    expect(specials.map((i) => i.id).sort()).toEqual(["2", "4"]);
  });

  it("treats empty special_type string as main-run", () => {
    const items = [issue({ id: "1", special_type: "" })];
    const { mainRun, specials } = splitMainAndSpecials(items);
    expect(mainRun).toHaveLength(1);
    expect(specials).toHaveLength(0);
  });
});

describe("sortSpecials", () => {
  it("orders by special_type ascending, then title/number/id", () => {
    const items = [
      issue({ id: "z", special_type: "OneShot", title: "Z" }),
      issue({ id: "a", special_type: "Annual", title: "Annual A" }),
      issue({ id: "b", special_type: "Annual", title: "Annual B" }),
      issue({ id: "s", special_type: "Special", title: "Sp" }),
    ];
    const out = sortSpecials(items).map((i) => i.id);
    // Annual A, Annual B, OneShot Z, Special Sp
    expect(out).toEqual(["a", "b", "z", "s"]);
  });

  it("is stable across re-runs with identical input", () => {
    const items = [
      issue({ id: "1", special_type: "Annual", title: "X" }),
      issue({ id: "2", special_type: "Annual", title: "X" }),
      issue({ id: "3", special_type: "Annual", title: "X" }),
    ];
    const a = sortSpecials(items).map((i) => i.id);
    const b = sortSpecials(items).map((i) => i.id);
    expect(a).toEqual(b);
  });

  it("falls back to number when title is missing", () => {
    const items = [
      issue({ id: "b", special_type: "Annual", title: null, number: "2" }),
      issue({ id: "a", special_type: "Annual", title: null, number: "1" }),
    ];
    expect(sortSpecials(items).map((i) => i.id)).toEqual(["a", "b"]);
  });

  it("does not mutate the input array", () => {
    const items = [
      issue({ id: "z", special_type: "OneShot" }),
      issue({ id: "a", special_type: "Annual" }),
    ];
    const originalOrder = items.map((i) => i.id);
    sortSpecials(items);
    expect(items.map((i) => i.id)).toEqual(originalOrder);
  });
});

describe("<SpecialsExtrasSection>", () => {
  function render(items: IssueSummaryView[]): string {
    return renderToStaticMarkup(
      createElement(SpecialsExtrasSection, {
        items,
        gridStyle: { gridTemplateColumns: "1fr" },
      }),
    );
  }

  it("renders nothing when items is empty", () => {
    expect(render([])).toBe("");
  });

  it("renders heading + grid when populated", () => {
    const html = render([
      issue({
        id: "a",
        slug: "annual-1",
        special_type: "Annual",
        title: "Annual 1",
      }),
    ]);
    expect(html).toContain("Specials &amp; Extras");
    expect(html).toContain("data-testid=\"specials-extras-section\"");
    expect(html).toContain("Annual 1");
  });
});
