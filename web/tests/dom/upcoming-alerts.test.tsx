// @vitest-environment jsdom
/**
 * The in-app alert surfaces of upcoming releases (upcoming-releases 1.0):
 * the "Out this week" home rail and the series page's "Next issue" fact.
 * Both only report, and both render nothing when there is nothing to say
 * (which is also how they disappear while the feature is off).
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { UpcomingEntryView } from "@/lib/api/types";

const state = vi.hoisted(() => ({
  rail: { isPending: false, isLoading: false, isError: false } as Record<
    string,
    unknown
  >,
  next: { data: undefined } as Record<string, unknown>,
  railCalls: [] as { today: string; enabled: boolean | undefined }[],
}));

// The card's cover frame reads the account's effect switch (`useMe`).
vi.mock("@/lib/visual-effects", () => ({
  useVisualEffect: () => false,
  useReducedMotion: () => true,
}));
vi.mock("@/lib/api/queries", () => ({
  useUpcomingThisWeek: (today: string, opts?: { enabled?: boolean }) => {
    state.railCalls.push({ today, enabled: opts?.enabled });
    return state.rail;
  },
  useSeriesNextRelease: () => state.next,
  // The other system rails `useSystemRailIsEmpty` asks about.
  useContinueReading: () => ({ isLoading: false, data: undefined }),
  useOnDeck: () => ({ isLoading: false, data: undefined }),
  useRecentIssues: () => ({ isLoading: false, data: undefined }),
  useSimilarRailInfinite: () => ({ isLoading: false, data: undefined }),
  useRecentHistory: () => ({ isLoading: false, data: undefined }),
  useRandomPicks: () => ({ isLoading: false, data: undefined }),
  useOnThisDay: () => ({ isLoading: false, data: undefined }),
  useNewInSeriesInfinite: () => ({ isLoading: false, data: undefined }),
}));

import { useSystemRailIsEmpty } from "@/components/saved-views/system-rails";
import { SeriesNextIssueFact } from "@/components/upcoming/SeriesNextIssueFact";
import { UpcomingThisWeekRailBody } from "@/components/upcoming/UpcomingRail";

function entry(over: Partial<UpcomingEntryView> = {}): UpcomingEntryView {
  return {
    key: "m1",
    source: "metron",
    series_name: "Saga",
    number: "73",
    publisher: "Image Comics",
    release_date: "2026-11-18",
    cover_date: null,
    cover_url: null,
    metron_url: null,
    first_issue: false,
    library_series: { slug: "saga", name: "Saga" },
    owned: false,
    tracked: true,
    mylar_status: null,
    stage: "tracked",
    ...over,
  };
}

const loaded = (items: UpcomingEntryView[]) => ({
  isPending: false,
  isLoading: false,
  isError: false,
  data: { items },
});

beforeEach(() => {
  state.rail = loaded([]);
  state.next = { data: undefined };
  state.railCalls = [];
});

describe("Out this week rail", () => {
  it("draws this week's releases and opens the read-only dialog", () => {
    state.rail = loaded([entry(), entry({ key: "m2", number: "74" })]);
    render(<UpcomingThisWeekRailBody itemStyle={{}} />);
    const cards = document.querySelectorAll("[data-card]");
    expect(cards).toHaveLength(2);
    expect(cards[0]?.querySelector("[data-stage]")?.textContent).toBe(
      "Tracked",
    );
    fireEvent.click(cards[0] as HTMLElement);
    const dialog = within(screen.getByRole("dialog"));
    expect(dialog.getByText("Saga #73")).toBeTruthy();
    expect(dialog.getAllByRole("button").map((b) => b.textContent)).toEqual([
      "Close",
    ]);
  });

  it("asks with the viewer's local date", () => {
    render(<UpcomingThisWeekRailBody itemStyle={{}} />);
    const call = state.railCalls.at(-1)!;
    expect(call.today).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    expect(call.enabled).toBe(true);
  });

  it("hides when empty, stays while loading or failed", () => {
    const isEmpty = () =>
      renderHook(() => useSystemRailIsEmpty("upcoming_this_week")).result
        .current;
    expect(isEmpty()).toBe(true);
    state.rail = loaded([entry()]);
    expect(isEmpty()).toBe(false);
    // Not run yet: not empty yet.
    state.rail = { isPending: true, isLoading: false, isError: false };
    expect(isEmpty()).toBe(false);
    state.rail = { isPending: false, isLoading: false, isError: true };
    expect(isEmpty()).toBe(false);
    // Another rail's key never reads this one as its own.
    state.rail = loaded([]);
    expect(
      renderHook(() => useSystemRailIsEmpty("some_other_rail")).result.current,
    ).toBe(false);
  });
});

describe("Next issue on the series page", () => {
  it("names the next issue the library does not hold yet", () => {
    state.next = {
      data: {
        entries: [
          entry({ number: "73", owned: true, stage: "in_library" }),
          entry({
            key: "m2",
            number: "74",
            release_date: "2026-12-16",
            mylar_status: "wanted",
            stage: "wanted",
          }),
        ],
      },
    };
    render(<SeriesNextIssueFact slug="saga" />);
    const fact = within(screen.getByTestId("series-next-issue"));
    const link = fact.getByRole("link");
    expect(link.textContent).toMatch(/^Next issue #74 · Wed 16 Dec/);
    expect(link.getAttribute("href")).toBe("/upcoming");
    expect(fact.getByText("Wanted")).toBeTruthy();
  });

  it("carries no badge for a release nobody tracks", () => {
    state.next = {
      data: { entries: [entry({ tracked: false, stage: "announced" })] },
    };
    render(<SeriesNextIssueFact slug="saga" />);
    expect(
      screen.getByTestId("series-next-issue").querySelector("[data-stage]"),
    ).toBeNull();
  });

  it("renders nothing when nothing is announced or all of it is owned", () => {
    state.next = { data: { entries: [] } };
    const { container, rerender } = render(<SeriesNextIssueFact slug="saga" />);
    expect(container.innerHTML).toBe("");
    state.next = {
      data: { entries: [entry({ owned: true, stage: "in_library" })] },
    };
    rerender(<SeriesNextIssueFact slug="saga" />);
    expect(container.innerHTML).toBe("");
  });
});
