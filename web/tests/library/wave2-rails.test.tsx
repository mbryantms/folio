/**
 * Wave-2 system rails (filter-parity-and-builtin-views-1.0 M6): Recently
 * read / Random picks / On this day. Same idiom as
 * `recent-issues-rail.test.tsx` — mock the query hooks, invoke the rail
 * body, walk the element tree by component type.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";

const mockUseRecentHistory = vi.fn();
const mockUseRandomPicks = vi.fn();
const mockUseOnThisDay = vi.fn();
vi.mock("@/lib/api/queries", () => ({
  useContinueReading: () => ({ isLoading: false, data: { items: [] } }),
  useOnDeck: () => ({ isLoading: false, data: { items: [] } }),
  useRecentIssues: () => ({ isLoading: false, data: { items: [] } }),
  useSimilarRailInfinite: () => ({ isLoading: false, data: undefined }),
  useRecentHistory: (...args: unknown[]) => mockUseRecentHistory(...args),
  useRandomPicks: (...args: unknown[]) => mockUseRandomPicks(...args),
  useOnThisDay: (...args: unknown[]) => mockUseOnThisDay(...args),
}));
vi.mock("@/lib/api/mutations", () => ({
  useDismissRailItem: () => ({ mutate: vi.fn() }),
}));

import { IssueCard, IssueCardSkeleton } from "@/components/library/IssueCard";
import {
  ProgressIssueCard,
  ProgressIssueCardSkeleton,
} from "@/components/library/ProgressIssueCard";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import {
  OnThisDayRailBody,
  RandomPicksRailBody,
  RecentHistoryRailBody,
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
const ok = (items: unknown[]) => ({ isLoading: false, data: { items } });

describe("RecentHistoryRailBody", () => {
  it("renders skeletons while loading, null when empty, one progress card per item", () => {
    mockUseRecentHistory.mockReturnValue({ isLoading: true, data: undefined });
    expect(
      collectByType(
        RecentHistoryRailBody({ itemStyle }),
        ProgressIssueCardSkeleton,
      ).length,
    ).toBeGreaterThan(0);
    mockUseRecentHistory.mockReturnValue(ok([]));
    expect(RecentHistoryRailBody({ itemStyle })).toBeNull();
    const card = {
      issue: issue(),
      series_name: "Invincible",
      progress: {
        last_page: 19,
        percent: 1,
        updated_at: "2026-07-01T00:00:00Z",
      },
    };
    mockUseRecentHistory.mockReturnValue(
      ok([card, { ...card, issue: issue({ id: "i2" }) }]),
    );
    expect(
      collectByType(RecentHistoryRailBody({ itemStyle }), ProgressIssueCard),
    ).toHaveLength(2);
  });
});

describe("RandomPicksRailBody", () => {
  it("renders series cards from the daily shuffle", () => {
    mockUseRandomPicks.mockReturnValue({ isLoading: true, data: undefined });
    expect(
      collectByType(RandomPicksRailBody({ itemStyle }), SeriesCardSkeleton)
        .length,
    ).toBeGreaterThan(0);
    mockUseRandomPicks.mockReturnValue(ok([]));
    expect(RandomPicksRailBody({ itemStyle })).toBeNull();
    mockUseRandomPicks.mockReturnValue(
      ok([
        { id: "s1", name: "Saga" },
        { id: "s2", name: "Paper Girls" },
      ]),
    );
    expect(
      collectByType(RandomPicksRailBody({ itemStyle }), SeriesCard),
    ).toHaveLength(2);
  });
});

describe("OnThisDayRailBody", () => {
  it("renders issue cards for today's releases", () => {
    mockUseOnThisDay.mockReturnValue({ isLoading: true, data: undefined });
    expect(
      collectByType(OnThisDayRailBody({ itemStyle }), IssueCardSkeleton).length,
    ).toBeGreaterThan(0);
    mockUseOnThisDay.mockReturnValue(ok([]));
    expect(OnThisDayRailBody({ itemStyle })).toBeNull();
    mockUseOnThisDay.mockReturnValue(ok([issue(), issue({ id: "i2" })]));
    expect(
      collectByType(OnThisDayRailBody({ itemStyle }), IssueCard),
    ).toHaveLength(2);
  });
});

describe("useSystemRailIsEmpty (wave 2)", () => {
  it("reports empty only when the matching rail loaded with no items", () => {
    mockUseRecentHistory.mockReturnValue(ok([]));
    mockUseRandomPicks.mockReturnValue({ isLoading: true, data: undefined });
    mockUseOnThisDay.mockReturnValue({
      isLoading: false,
      isError: true,
      data: undefined,
    });
    expect(useSystemRailIsEmpty("recent_history")).toBe(true);
    expect(useSystemRailIsEmpty("random_picks")).toBe(false);
    expect(useSystemRailIsEmpty("on_this_day")).toBe(false);
  });
});
