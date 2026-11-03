// @vitest-environment jsdom
/**
 * Start Here rail (Start Here + default pins M2):
 *
 *  - `useSystemRailIsEmpty("start_here")` hides the rail only once the
 *    featured set loaded empty (never while loading or on error).
 *  - `StartHereCard` links to the list and words progress per state:
 *    issue count, partial availability, or "n of m read".
 */
import { render, renderHook, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { StartHereItemView } from "@/lib/api/types";

const mockUseStartHere = vi.fn();
vi.mock("@/lib/api/queries", () => {
  const idle = () => ({ isLoading: false, data: { items: [] } });
  return {
    useContinueReading: idle,
    useOnDeck: idle,
    useRecentIssues: idle,
    useSimilarRailInfinite: () => ({ isLoading: false, data: undefined }),
    useRecentHistory: idle,
    useRandomPicks: idle,
    useOnThisDay: idle,
    useStartHere: (...args: unknown[]) => mockUseStartHere(...args),
  };
});

import { useSystemRailIsEmpty } from "@/components/saved-views/system-rails";
import { StartHereCard } from "@/components/saved-views/StartHereCard";

function item(overrides: Partial<StartHereItemView> = {}): StartHereItemView {
  return {
    view_id: "v1",
    cbl_list_id: "l1",
    name: "Intro to Marvel",
    description: "Where to begin.",
    total: 40,
    available: 40,
    read_count: 0,
    cover_issue_ids: ["a", "b", "c"],
    ...overrides,
  };
}

describe("useSystemRailIsEmpty(start_here)", () => {
  it("is empty only when the featured set loaded with no items", () => {
    mockUseStartHere.mockReturnValue({
      isLoading: false,
      isError: false,
      data: { items: [] },
    });
    expect(
      renderHook(() => useSystemRailIsEmpty("start_here")).result.current,
    ).toBe(true);

    mockUseStartHere.mockReturnValue({
      isLoading: true,
      isError: false,
      data: undefined,
    });
    expect(
      renderHook(() => useSystemRailIsEmpty("start_here")).result.current,
    ).toBe(false);

    mockUseStartHere.mockReturnValue({
      isLoading: false,
      isError: true,
      data: undefined,
    });
    expect(
      renderHook(() => useSystemRailIsEmpty("start_here")).result.current,
    ).toBe(false);

    mockUseStartHere.mockReturnValue({
      isLoading: false,
      isError: false,
      data: { items: [item()] },
    });
    expect(
      renderHook(() => useSystemRailIsEmpty("start_here")).result.current,
    ).toBe(false);
  });

  it("only fetches for the start_here key", () => {
    mockUseStartHere.mockClear();
    mockUseStartHere.mockReturnValue({
      isLoading: false,
      isError: false,
      data: { items: [] },
    });
    renderHook(() => useSystemRailIsEmpty("on_deck"));
    expect(mockUseStartHere).toHaveBeenLastCalledWith({ enabled: false });
  });
});

describe("StartHereCard", () => {
  it("links to the reading list and shows the issue count when unstarted", () => {
    render(<StartHereCard item={item()} />);
    expect(screen.getByRole("link").getAttribute("href")).toBe("/views/v1");
    expect(screen.getByText("40 issues")).toBeTruthy();
    expect(screen.getByText("Where to begin.")).toBeTruthy();
  });

  it("says how much of the list is in the library", () => {
    render(<StartHereCard item={item({ available: 25 })} />);
    expect(screen.getByText("25 of 40 issues in the library")).toBeTruthy();
  });

  it("shows read progress once started", () => {
    render(<StartHereCard item={item({ available: 25, read_count: 5 })} />);
    expect(screen.getByText("5 of 25 read")).toBeTruthy();
  });
});
