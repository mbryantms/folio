/**
 * Issue-level (`filter_issues`) saved-view rail body — WP-5.4. A pinned
 * issue view (e.g. "Unread annuals 2019") renders one `IssueCard` per
 * `issue-results` item. Node-env element-tree walk, same idiom as
 * `recent-issues-rail.test.tsx`.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";

const mockIssueResults = vi.fn();
vi.mock("@/lib/api/queries", () => ({
  useSavedViewIssueResults: (...args: unknown[]) => mockIssueResults(...args),
  useSavedViewResults: () => ({ isLoading: false, data: { items: [] } }),
  useCblListWindowInfinite: () => ({}),
  useCollectionEntries: () => ({}),
  useContinueReading: () => ({ isLoading: false, data: { items: [] } }),
  useOnDeck: () => ({ isLoading: false, data: { items: [] } }),
  useRecentIssues: () => ({ isLoading: false, data: { items: [] } }),
}));

import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import { IssueFilterRailBody } from "@/components/saved-views/SavedViewRail";
import type { IssueSummaryView, SavedViewView } from "@/lib/api/types";

const VIEW: SavedViewView = {
  id: "v-annuals",
  kind: "filter_issues",
  user_id: "u1",
  is_system: false,
  name: "Unread annuals 2019",
  description: null,
  custom_year_start: null,
  custom_year_end: null,
  custom_tags: [],
  match_mode: "all",
  conditions: [
    { field: "special_type", op: "is", value: "Annual" },
    { field: "year", op: "equals", value: 2019 },
    { field: "read_status", op: "is", value: "unread" },
  ],
  sort_field: "name",
  sort_order: "asc",
  result_limit: 12,
  cbl_list_id: null,
  pinned: true,
  pinned_position: 0,
  show_in_sidebar: false,
  pinned_on_pages: [],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

function issue(overrides: Partial<IssueSummaryView> = {}): IssueSummaryView {
  return {
    id: "i1",
    slug: "annual-1",
    series_id: "s1",
    series_slug: "batman",
    series_name: "Batman",
    title: null,
    number: "1",
    sort_number: 1,
    year: 2019,
    page_count: 40,
    state: "active",
    cover_url: "/issues/i1/pages/0/thumb",
    special_type: "Annual",
    created_at: "2026-07-01T00:00:00Z",
    updated_at: "2026-07-01T00:00:00Z",
    ...overrides,
  };
}

function collectByType(node: React.ReactNode, type: unknown): unknown[] {
  const found: unknown[] = [];
  const stack: React.ReactNode[] = [node];
  while (stack.length) {
    const cur = stack.shift();
    if (Array.isArray(cur)) {
      stack.push(...cur);
      continue;
    }
    if (!cur || typeof cur !== "object" || !("type" in cur)) continue;
    const el = cur as React.ReactElement<{ children?: React.ReactNode }>;
    if (el.type === type) found.push(el);
    if (el.props && el.props.children) stack.push(el.props.children);
  }
  return found;
}

const itemStyle: React.CSSProperties = { width: "160px" };

describe("IssueFilterRailBody", () => {
  it("fetches the view's issue-results and shows skeletons while loading", () => {
    mockIssueResults.mockReturnValue({ isLoading: true, data: undefined });
    const tree = IssueFilterRailBody({ view: VIEW, itemStyle });
    expect(mockIssueResults).toHaveBeenCalledWith("v-annuals");
    expect(collectByType(tree, IssueCardSkeleton).length).toBeGreaterThan(0);
  });

  it("renders one IssueCard per matching issue", () => {
    mockIssueResults.mockReturnValue({
      isLoading: false,
      data: {
        items: [issue(), issue({ id: "i2", number: "2" })],
        next_cursor: null,
      },
    });
    const tree = IssueFilterRailBody({ view: VIEW, itemStyle });
    const cards = collectByType(tree, IssueCard) as Array<
      React.ReactElement<{ issue: IssueSummaryView }>
    >;
    expect(cards.map((c) => c.props.issue.id)).toEqual(["i1", "i2"]);
  });

  it("shows no cards when nothing matches", () => {
    mockIssueResults.mockReturnValue({
      isLoading: false,
      data: { items: [], next_cursor: null },
    });
    const tree = IssueFilterRailBody({ view: VIEW, itemStyle });
    expect(collectByType(tree, IssueCard)).toHaveLength(0);
  });
});
