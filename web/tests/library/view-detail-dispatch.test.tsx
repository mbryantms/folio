/**
 * Smoke checks for the saved-view detail page kind dispatch. We can't
 * render the full tree (vitest runs in node env) so we compare the
 * React element types in the returned tree against the imported
 * detail components — that's enough to confirm the kind branching
 * picks the right child.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";

// Stub the heavy children: importing the real ones would pull in
// TanStack hooks + dnd-kit + @tanstack/react-virtual, none of which
// run in a bare node env.
vi.mock("@/components/saved-views/CblViewDetail", () => ({
  CblViewDetail: function CblViewDetail() {
    return null;
  },
}));
vi.mock("@/components/saved-views/FilterViewDetail", () => ({
  FilterViewDetail: function FilterViewDetail() {
    return null;
  },
}));
vi.mock("@/components/saved-views/CollectionViewDetail", () => ({
  CollectionViewDetail: function CollectionViewDetail() {
    return null;
  },
}));

import { ViewClient } from "@/app/[locale]/(library)/views/[id]/ViewClient";
import { CblViewDetail } from "@/components/saved-views/CblViewDetail";
import { CollectionViewDetail } from "@/components/saved-views/CollectionViewDetail";
import { FilterViewDetail } from "@/components/saved-views/FilterViewDetail";
import type { SavedViewView } from "@/lib/api/types";

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
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function findByType(node: React.ReactNode, type: unknown): unknown {
  const stack: React.ReactNode[] = [node];
  while (stack.length) {
    const cur = stack.shift();
    if (Array.isArray(cur)) {
      stack.push(...cur);
      continue;
    }
    if (!cur || typeof cur !== "object" || !("type" in cur)) continue;
    const el = cur as React.ReactElement<{ children?: React.ReactNode }>;
    if (el.type === type) return el;
    if (el.props && el.props.children) stack.push(el.props.children);
  }
  return null;
}

describe("ViewClient kind dispatch", () => {
  it("renders FilterViewDetail for filter_series kind", () => {
    const tree = ViewClient({ view: view() });
    expect(findByType(tree, FilterViewDetail)).toBeTruthy();
    expect(findByType(tree, CblViewDetail)).toBeFalsy();
  });

  it("renders CblViewDetail for cbl kind", () => {
    const tree = ViewClient({
      view: view({ kind: "cbl", cbl_list_id: "list-1" }),
    });
    expect(findByType(tree, CblViewDetail)).toBeTruthy();
    expect(findByType(tree, FilterViewDetail)).toBeFalsy();
  });

  it("renders CollectionViewDetail for collection kind", () => {
    const tree = ViewClient({
      view: view({
        kind: "collection",
        match_mode: null,
        conditions: null,
        sort_field: null,
        sort_order: null,
        result_limit: null,
      }),
    });
    expect(findByType(tree, CollectionViewDetail)).toBeTruthy();
    expect(findByType(tree, FilterViewDetail)).toBeFalsy();
    expect(findByType(tree, CblViewDetail)).toBeFalsy();
  });
});
