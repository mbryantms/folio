// @vitest-environment jsdom
/**
 * Post-audit T2.4: a rail whose fetch failed is not an empty rail. The
 * body shows a retry state, and the parent's "is empty → hide the rail"
 * probe answers false so the retry state is reachable.
 */
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, renderHook, screen } from "@testing-library/react";

const refetch = vi.fn();
const failed = () => ({
  isLoading: false,
  isError: true,
  data: undefined,
  refetch,
});
vi.mock("@/lib/api/queries", () => ({
  useContinueReading: () => failed(),
  useOnDeck: () => failed(),
  useRecentIssues: () => failed(),
  useSimilarRailInfinite: () => failed(),
}));
vi.mock("@/components/library/ProgressIssueCard", () => ({
  ProgressIssueCard: () => null,
  ProgressIssueCardSkeleton: () => null,
}));
vi.mock("@/components/library/OnDeckCard", () => ({
  OnDeckCard: () => null,
  OnDeckCardSkeleton: () => null,
}));
vi.mock("@/components/library/IssueCard", () => ({
  IssueCard: () => null,
  IssueCardSkeleton: () => null,
}));
vi.mock("@/components/library/SeriesCard", () => ({
  SeriesCardSkeleton: () => null,
}));
vi.mock("@/components/library/SimilarSeriesCard", () => ({
  SimilarSeriesCard: () => null,
}));

import {
  ContinueReadingRailBody,
  OnDeckRailBody,
  RecentIssuesRailBody,
  SimilarSeriesRailBody,
  useSystemRailIsEmpty,
} from "@/components/saved-views/system-rails";

describe("system rails on fetch failure", () => {
  it.each([
    ["continue reading", ContinueReadingRailBody],
    ["on deck", OnDeckRailBody],
    ["new issues", RecentIssuesRailBody],
    ["similar series", SimilarSeriesRailBody],
  ])("%s body shows a retry state that refetches", (_name, Body) => {
    refetch.mockClear();
    const { unmount } = render(<Body itemStyle={{}} />);
    expect(screen.getByRole("alert").textContent).toMatch(/Couldn't load/);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(refetch).toHaveBeenCalledTimes(1);
    unmount();
  });

  it.each(["continue_reading", "on_deck", "new_issues", "similar_series"])(
    "useSystemRailIsEmpty(%s) is false on error so the rail stays mounted",
    (key) => {
      const { result } = renderHook(() => useSystemRailIsEmpty(key));
      expect(result.current).toBe(false);
    },
  );
});
