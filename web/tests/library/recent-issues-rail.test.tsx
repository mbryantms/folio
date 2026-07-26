/**
 * New Issues rail body — branch behavior for the `new_issues` system
 * rail (loading skeletons / hide-when-empty / one `IssueCard` per
 * item). Rendered as element trees in node env (no DOM), same idiom
 * as `on-deck-card.test.tsx`: mock the query hook, invoke the body,
 * and walk the returned tree by component type.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";

const mockUseRecentIssues = vi.fn();
vi.mock("@/lib/api/queries", () => ({
  useContinueReading: () => ({ isLoading: false, data: { items: [] } }),
  useOnDeck: () => ({ isLoading: false, data: { items: [] } }),
  useRecentIssues: (...args: unknown[]) => mockUseRecentIssues(...args),
}));

import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import {
  RecentIssuesRailBody,
  useSystemRailIsEmpty,
} from "@/components/saved-views/system-rails";
import type { IssueSummaryView } from "@/lib/api/types";

function issue(overrides: Partial<IssueSummaryView> = {}): IssueSummaryView {
  return {
    id: "i1",
    slug: "1",
    series_id: "s1",
    series_slug: "invincible",
    series_name: "Invincible",
    title: null,
    number: "1",
    sort_number: 1,
    year: 2020,
    page_count: 20,
    state: "active",
    cover_url: "/issues/i1/pages/0/thumb",
    special_type: null,
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

describe("RecentIssuesRailBody", () => {
  it("renders skeletons while loading", () => {
    mockUseRecentIssues.mockReturnValue({ isLoading: true, data: undefined });
    const tree = RecentIssuesRailBody({ itemStyle });
    expect(collectByType(tree, IssueCardSkeleton).length).toBeGreaterThan(0);
    expect(collectByType(tree, IssueCard)).toHaveLength(0);
  });

  it("returns null when empty so the parent suppresses the section", () => {
    mockUseRecentIssues.mockReturnValue({
      isLoading: false,
      data: { items: [] },
    });
    expect(RecentIssuesRailBody({ itemStyle })).toBeNull();
  });

  it("renders one IssueCard per item", () => {
    mockUseRecentIssues.mockReturnValue({
      isLoading: false,
      data: { items: [issue(), issue({ id: "i2", number: "2" })] },
    });
    const tree = RecentIssuesRailBody({ itemStyle });
    const cards = collectByType(tree, IssueCard) as Array<
      React.ReactElement<{ issue: IssueSummaryView }>
    >;
    expect(cards).toHaveLength(2);
    expect(cards[0]!.props.issue.id).toBe("i1");
    expect(cards[1]!.props.issue.id).toBe("i2");
  });
});

describe("useSystemRailIsEmpty — new_issues branch", () => {
  it("reports empty only when loaded with zero items", () => {
    mockUseRecentIssues.mockReturnValue({ isLoading: true, data: undefined });
    expect(useSystemRailIsEmpty("new_issues")).toBe(false);

    mockUseRecentIssues.mockReturnValue({
      isLoading: false,
      data: { items: [] },
    });
    expect(useSystemRailIsEmpty("new_issues")).toBe(true);

    mockUseRecentIssues.mockReturnValue({
      isLoading: false,
      data: { items: [issue()] },
    });
    expect(useSystemRailIsEmpty("new_issues")).toBe(false);
  });

  it("gates the fetch on the matching system key", () => {
    mockUseRecentIssues.mockClear();
    mockUseRecentIssues.mockReturnValue({ isLoading: true, data: undefined });
    useSystemRailIsEmpty("continue_reading");
    expect(mockUseRecentIssues).toHaveBeenCalledWith({ enabled: false });
    useSystemRailIsEmpty("new_issues");
    expect(mockUseRecentIssues).toHaveBeenLastCalledWith({ enabled: true });
  });
});
