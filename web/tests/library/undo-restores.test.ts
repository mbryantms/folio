import { describe, expect, it, vi } from "vitest";

import type * as Core from "@/lib/api/mutations/_core";

const { apiMutate } = vi.hoisted(() => ({
  apiMutate: vi.fn(async (..._args: unknown[]) => ({ id: "new" })),
}));
vi.mock("@/lib/api/mutations/_core", async (importOriginal) => ({
  ...(await importOriginal<typeof Core>()),
  apiMutate,
}));

import { restoredGroupLayout } from "@/components/saved-views/GroupedViewsBoard";
import { recreateSavedView } from "@/components/saved-views/ViewsIndex";
import type { SavedViewView, ViewGroupListView } from "@/lib/api/types";

const group = (
  id: string,
  view_ids: string[],
  position = 0,
  parent_id: string | null = null,
) =>
  ({
    id,
    view_ids,
    position,
    parent_id,
  }) as ViewGroupListView["groups"][number];

describe("restoredGroupLayout", () => {
  it("puts the re-created group back in the deleted one's slot", () => {
    const before = {
      groups: [
        group("g1", ["a"], 0),
        group("old", ["b", "c"], 1),
        group("g3", [], 2),
      ],
    };
    // After the delete its lists joined the ungrouped order; the
    // re-create appended the new group at the end.
    const current = {
      groups: [group("g1", ["a"], 0), group("g3", [], 1), group("new", [], 2)],
      ungrouped_ids: ["d", "e", "b", "c"],
    } as ViewGroupListView;
    expect(
      restoredGroupLayout("filter", before, "old", "new", current),
    ).toEqual({
      scope: "filter",
      groups: [
        { id: "g1", parent_id: null, view_ids: ["a"] },
        { id: "new", parent_id: null, view_ids: ["b", "c"] },
        { id: "g3", parent_id: null, view_ids: [] },
      ],
      ungrouped: ["d", "e"],
    });
  });

  it("restores a nested group's parent, slot, lists and child groups", () => {
    // R ⊃ [old ⊃ [C (c)] (a), S]  (x in R)
    const before = {
      groups: [
        group("R", ["x"], 0),
        group("old", ["a"], 0, "R"),
        group("S", [], 1, "R"),
        group("C", ["c"], 0, "old"),
      ],
    };
    // After the delete: R ⊃ [C, S] with a appended to R; the new group
    // is top-level.
    const current = {
      groups: [
        group("R", ["x", "a"], 0),
        group("C", ["c"], 0, "R"),
        group("S", [], 1, "R"),
        group("new", [], 1),
      ],
      ungrouped_ids: [],
    } as unknown as ViewGroupListView;
    expect(restoredGroupLayout("cbl", before, "old", "new", current)).toEqual({
      scope: "cbl",
      groups: [
        { id: "R", parent_id: null, view_ids: ["x"] },
        { id: "new", parent_id: "R", view_ids: ["a"] },
        { id: "C", parent_id: "new", view_ids: ["c"] },
        { id: "S", parent_id: "R", view_ids: [] },
      ],
      ungrouped: [],
    });
  });

  it("keeps groups made since, skips groups deleted since, and drops ungrouped ids now in a group", () => {
    const before = {
      groups: [
        group("P", [], 0),
        group("old", ["b"], 0, "P"),
        group("K", [], 0, "old"),
      ],
    };
    // Since: P was deleted too (old's slot now hangs from the top), g9
    // was made and took "e".
    const current = {
      groups: [group("K", [], 0), group("g9", ["e"], 1), group("new", [], 2)],
      ungrouped_ids: ["d", "e", "b"],
    } as ViewGroupListView;
    const out = restoredGroupLayout("cbl", before, "old", "new", current);
    expect(out.groups).toEqual([
      { id: "new", parent_id: null, view_ids: ["b"] },
      { id: "K", parent_id: "new", view_ids: [] },
      { id: "g9", parent_id: null, view_ids: ["e"] },
    ]);
    expect(out.ungrouped).toEqual(["d"]);
  });
});

describe("recreateSavedView", () => {
  const base = {
    id: "v1",
    name: "Big events",
    description: "desc",
    custom_tags: ["event"],
    custom_year_start: null,
    custom_year_end: null,
  };

  it("re-posts a filter view's definition", async () => {
    apiMutate.mockClear();
    await recreateSavedView({
      ...base,
      kind: "filter_series",
      match_mode: "any",
      conditions: [{ field: "publisher", op: "equals", value: "DC" }],
      sort_field: "name",
      sort_order: "asc",
      result_limit: 12,
    } as unknown as SavedViewView);
    const arg = apiMutate.mock.calls[0]?.[0] as {
      path: string;
      method: string;
      body: Record<string, unknown>;
    };
    expect(arg.path).toBe("/me/saved-views");
    expect(arg.method).toBe("POST");
    expect(arg.body).toMatchObject({
      kind: "filter_series",
      name: "Big events",
      description: "desc",
      custom_tags: ["event"],
      filter: {
        match_mode: "any",
        conditions: [{ field: "publisher", op: "equals", value: "DC" }],
      },
      sort_field: "name",
      sort_order: "asc",
      result_limit: 12,
    });
    expect(arg.body).not.toHaveProperty("cbl_list_id");
  });

  it("re-points a CBL view at the same list", async () => {
    apiMutate.mockClear();
    await recreateSavedView({
      ...base,
      kind: "cbl",
      cbl_list_id: "list-1",
    } as unknown as SavedViewView);
    const arg = apiMutate.mock.calls[0]?.[0] as {
      body: Record<string, unknown>;
    };
    expect(arg.body).toMatchObject({ kind: "cbl", cbl_list_id: "list-1" });
    expect(arg.body).not.toHaveProperty("filter");
  });
});
