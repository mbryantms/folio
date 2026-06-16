/**
 * `splitSavedViews` — the kind-filter behind the unified /views index (A3).
 * Guards the invariant that system rails (continue_reading / on_deck) and
 * collections never leak into the Filter views or Reading lists sections.
 */
import { describe, expect, it } from "vitest";

import { splitSavedViews } from "@/components/saved-views/ViewsIndex";
import type { SavedViewView } from "@/lib/api/types";

function v(id: string, kind: SavedViewView["kind"]): SavedViewView {
  return { id, kind } as unknown as SavedViewView;
}

describe("splitSavedViews", () => {
  const items = [
    v("f1", "filter_series"),
    v("rail", "system"),
    v("c1", "cbl"),
    v("col1", "collection"),
    v("f2", "filter_series"),
    v("c2", "cbl"),
  ];

  it("filter views are filter_series only — no system rails, cbl, or collections", () => {
    const { filterViews } = splitSavedViews(items);
    expect(filterViews.map((x) => x.id)).toEqual(["f1", "f2"]);
    expect(filterViews.every((x) => x.kind === "filter_series")).toBe(true);
  });

  it("reading lists are cbl only", () => {
    const { readingLists } = splitSavedViews(items);
    expect(readingLists.map((x) => x.id)).toEqual(["c1", "c2"]);
  });

  it("system + collection kinds appear in neither section", () => {
    const { filterViews, readingLists } = splitSavedViews(items);
    const ids = [...filterViews, ...readingLists].map((x) => x.id);
    expect(ids).not.toContain("rail");
    expect(ids).not.toContain("col1");
  });
});
