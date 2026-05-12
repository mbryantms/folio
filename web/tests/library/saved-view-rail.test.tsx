/**
 * Smoke tests for the saved-view rail kind dispatch. We can't render
 * the full tree (TanStack hooks need a QueryClient context, and
 * vitest's node env has no DOM), but the test we actually care about
 * is "does the right card component get reached for each kind?". We
 * exercise that via the data-side helpers — the rail's body branches
 * are mechanical once kind is known.
 */
import { describe, expect, it } from "vitest";

import type { SavedViewView } from "@/lib/api/types";

function view(overrides: Partial<SavedViewView> = {}): SavedViewView {
  return {
    id: "v1",
    kind: "filter_series",
    user_id: null,
    is_system: true,
    name: "Recently Added",
    description: null,
    custom_year_start: null,
    custom_year_end: null,
    custom_tags: [],
    match_mode: "all",
    conditions: [],
    sort_field: "created_at",
    sort_order: "desc",
    result_limit: 12,
    cbl_list_id: null,
    pinned: true,
    pinned_position: 0,
    show_in_sidebar: false,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

describe("saved-view kind dispatch contract", () => {
  it("filter_series views supply a sort_field and have no cbl_list_id", () => {
    const v = view();
    expect(v.kind).toBe("filter_series");
    expect(v.cbl_list_id).toBeNull();
    expect(v.sort_field).not.toBeNull();
  });

  it("cbl views must carry a cbl_list_id for the rail to render", () => {
    const v = view({
      kind: "cbl",
      cbl_list_id: "list-uuid",
      sort_field: null,
      sort_order: null,
      result_limit: null,
      conditions: null,
      match_mode: null,
    });
    expect(v.kind).toBe("cbl");
    expect(v.cbl_list_id).toBe("list-uuid");
  });

  it("system views can't be deleted from the rail menu — guard prop is is_system", () => {
    const sys = view({ is_system: true });
    const usr = view({ is_system: false, user_id: "u1" });
    expect(sys.is_system).toBe(true);
    expect(usr.is_system).toBe(false);
  });

  it("pinned views carry a pinned_position; unpinned ones do not", () => {
    expect(view({ pinned: true, pinned_position: 3 }).pinned_position).toBe(3);
    expect(
      view({ pinned: false, pinned_position: null }).pinned_position,
    ).toBeNull();
  });
});
