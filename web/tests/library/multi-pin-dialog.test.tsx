/**
 * Multi-page rails M6 — `<MultiPinDialog>` checkbox state contract.
 *
 * Vitest's node env can't fully render the dialog (it uses Radix
 * primitives that need a DOM + portal target), but we can verify the
 * data shape that feeds the picker: `view.pinned_on_pages` controls
 * which boxes start checked, and `pin_count >= 12` flips the
 * cap-disabled state.
 */
import { describe, expect, it } from "vitest";

import type { PageView, SavedViewView } from "@/lib/api/types";

function view(overrides: Partial<SavedViewView> = {}): SavedViewView {
  return {
    id: "v1",
    kind: "filter_series",
    user_id: "u1",
    is_system: false,
    name: "Horror picks",
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
    pinned: false,
    pinned_position: null,
    show_in_sidebar: false,
    pinned_on_pages: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function page(overrides: Partial<PageView> & Pick<PageView, "id">): PageView {
  return {
    name: "Page",
    slug: "page",
    is_system: false,
    position: 0,
    pin_count: 0,
    description: null,
    show_in_sidebar: true,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

describe("multi-pin dialog data contract", () => {
  it("pinned_on_pages drives initial checkbox state", () => {
    const v = view({ pinned_on_pages: ["page-a", "page-c"] });
    const set = new Set(v.pinned_on_pages);
    expect(set.has("page-a")).toBe(true);
    expect(set.has("page-b")).toBe(false);
    expect(set.has("page-c")).toBe(true);
  });

  it("pin_count >= 12 marks a page as cap-disabled for views not pinned on it", () => {
    const v = view({ pinned_on_pages: ["page-full"] });
    const full = page({ id: "page-full", pin_count: 12 });
    const otherFull = page({ id: "page-other", pin_count: 12 });
    const open = page({ id: "page-open", pin_count: 5 });

    // Pages the view is already on never count as cap-disabled — toggling
    // off doesn't increment the count.
    const fullDisabled = !v.pinned_on_pages.includes(full.id) && full.pin_count >= 12;
    const otherDisabled =
      !v.pinned_on_pages.includes(otherFull.id) && otherFull.pin_count >= 12;
    const openDisabled =
      !v.pinned_on_pages.includes(open.id) && open.pin_count >= 12;

    expect(fullDisabled).toBe(false); // already pinned → not disabled
    expect(otherDisabled).toBe(true); // full + not pinned → disabled
    expect(openDisabled).toBe(false); // has room → enabled
  });
});
