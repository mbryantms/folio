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

const group = (id: string, view_ids: string[]) =>
  ({ id, view_ids }) as ViewGroupListView["groups"][number];

describe("restoredGroupLayout", () => {
  it("puts the re-created group back in the deleted one's slot", () => {
    const before = {
      groups: [
        { id: "g1", view_ids: ["a"] },
        { id: "old", view_ids: ["b", "c"] },
        { id: "g3", view_ids: [] },
      ],
      ungrouped: ["d", "e"],
    };
    // After re-create the server appends the new group at the end.
    const current = {
      groups: [group("g1", ["a"]), group("g3", []), group("new", ["b", "c"])],
      ungrouped_ids: ["d", "e"],
    } as ViewGroupListView;
    expect(
      restoredGroupLayout("filter", before, "old", "new", current),
    ).toEqual({
      scope: "filter",
      groups: [
        { id: "g1", view_ids: ["a"] },
        { id: "new", view_ids: ["b", "c"] },
        { id: "g3", view_ids: [] },
      ],
      ungrouped: ["d", "e"],
    });
  });

  it("keeps groups made since and drops ungrouped ids now in a group", () => {
    const before = {
      groups: [{ id: "old", view_ids: ["b"] }],
      ungrouped: ["d", "e"],
    };
    const current = {
      groups: [group("g9", ["e"]), group("new", ["b"])],
      ungrouped_ids: ["d"],
    } as ViewGroupListView;
    const out = restoredGroupLayout("cbl", before, "old", "new", current);
    expect(out.groups.map((g) => g.id)).toEqual(["new", "g9"]);
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
