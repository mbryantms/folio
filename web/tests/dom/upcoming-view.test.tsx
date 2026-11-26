// @vitest-environment jsdom
/**
 * /upcoming (upcoming-releases 1.0). The page never narrows a list in
 * memory, so the tests pin:
 *  - every filter reaches the list hook as a server param;
 *  - a viewer who may not see Mylar data gets no Mylar control;
 *  - the detail dialog offers links only, except "Add series to Mylar"
 *    for an admin with add-to-Mylar set up, on an untracked series.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { UpcomingFilters } from "@/lib/api/query-keys";
import type {
  UpcomingEntryView,
  UpcomingSummaryView,
  UpcomingWeekView,
} from "@/lib/api/types";

const state = vi.hoisted(() => ({
  role: "admin" as "admin" | "user",
  summary: null as unknown,
  weeks: [] as unknown[],
  total: 0,
  filters: [] as unknown[],
}));

vi.mock("@/lib/api/queries-me", () => ({
  useMe: () => ({ data: { role: state.role } }),
}));
const addSeries = vi.fn();
vi.mock("@/lib/api/mutations", () => ({
  useAddSeriesToMylar: () => ({ mutate: addSeries, isPending: false }),
}));
vi.mock("@/lib/api/queries", () => ({
  useUpcomingSummary: () => ({
    isLoading: false,
    isError: false,
    data: state.summary,
  }),
  useUpcomingInfinite: (filters: UpcomingFilters) => {
    state.filters.push(filters);
    return {
      isLoading: false,
      isError: false,
      hasNextPage: false,
      isFetchingNextPage: false,
      isFetchNextPageError: false,
      fetchNextPage: async () => undefined,
      data: { pages: [{ items: state.weeks, total: state.total }] },
    };
  },
}));
// The cover-size control reads a browser store the page does not own.
vi.mock("@/components/library/CoverSizeOptions", () => ({
  CoverSizeOptions: () => null,
  CoverSizeFields: () => null,
}));

import { UpcomingView } from "@/components/upcoming/UpcomingView";
import { UPCOMING_LAYOUT_KEY } from "@/lib/upcoming/layout";

class IOStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

function summary(over: Partial<UpcomingSummaryView> = {}): UpcomingSummaryView {
  return {
    enabled: true,
    this_week: 2,
    next_week: 1,
    calendar_synced_at: "2026-11-14T05:20:00Z",
    mylar: { received_at: "2026-11-14T12:00:00Z", freshness: "fresh" },
    publishers: ["DC Comics", "Image"],
    all_releases_available: true,
    can_add_to_mylar: false,
    ...over,
  };
}

function entry(over: Partial<UpcomingEntryView> = {}): UpcomingEntryView {
  return {
    key: "m1",
    source: "metron",
    series_name: "Action Comics",
    number: "1104",
    publisher: "DC Comics",
    release_date: "2026-11-18",
    cover_date: "2027-01-01",
    cover_url: null,
    metron_url: "https://metron.cloud/issue/1/",
    first_issue: false,
    library_series: null,
    owned: false,
    tracked: false,
    mylar_status: null,
    stage: "announced",
    ...over,
  };
}

function week(entries: UpcomingEntryView[]): UpcomingWeekView {
  return { week_start: "2026-11-16", week_end: "2026-11-22", entries };
}

const lastFilters = () => state.filters.at(-1) as UpcomingFilters;
/** The `sm`-and-up toolbar (jsdom renders both compositions). */
const toolbar = () =>
  within(document.querySelector("[data-toolbar-row]") as HTMLElement);

beforeEach(() => {
  vi.stubGlobal("IntersectionObserver", IOStub);
  window.localStorage.removeItem(UPCOMING_LAYOUT_KEY);
  state.role = "admin";
  state.summary = summary();
  state.weeks = [];
  state.total = 0;
  state.filters = [];
});

describe("UpcomingView filters", () => {
  it("opens on My series and sends every filter to the server", () => {
    state.weeks = [week([entry()])];
    state.total = 1;
    render(<UpcomingView />);
    expect(lastFilters().scope).toBe("mine");
    expect(lastFilters().firstIssues).toBeUndefined();

    fireEvent.click(toolbar().getByRole("button", { name: "All releases" }));
    expect(lastFilters().scope).toBe("all");

    fireEvent.click(toolbar().getByRole("button", { name: "New #1s" }));
    fireEvent.click(toolbar().getByRole("button", { name: "Tracked" }));
    fireEvent.click(toolbar().getByRole("button", { name: "In library" }));
    fireEvent.change(toolbar().getByRole("combobox", { name: "Publisher" }), {
      target: { value: "Image" },
    });
    expect(lastFilters()).toMatchObject({
      scope: "all",
      firstIssues: true,
      tracked: true,
      inLibrary: true,
      publisher: "Image",
    });
  });

  it("offers a restricted viewer no Mylar or all-releases control", () => {
    state.role = "user";
    state.summary = summary({
      mylar: null,
      publishers: [],
      all_releases_available: false,
    });
    state.weeks = [week([entry({ library_series: { slug: "a", name: "A" } })])];
    render(<UpcomingView />);
    const bar = toolbar();
    expect(bar.queryByRole("button", { name: "All releases" })).toBeNull();
    expect(bar.queryByRole("button", { name: "Tracked" })).toBeNull();
    // Every release such a viewer sees is already in their library.
    expect(bar.queryByRole("button", { name: "In library" })).toBeNull();
    expect(bar.queryByRole("combobox", { name: "Publisher" })).toBeNull();
    expect(bar.getByRole("button", { name: "New #1s" })).toBeTruthy();
    expect(lastFilters().scope).toBe("mine");
    expect(screen.queryByText(/Mylar/)).toBeNull();
  });
});

describe("UpcomingView content", () => {
  it("groups releases by week and badges only what someone tracks", () => {
    state.weeks = [
      week([
        entry(),
        entry({
          key: "m2",
          series_name: "Saga",
          number: "73",
          publisher: "Image",
          tracked: true,
          stage: "tracked",
        }),
      ]),
    ];
    state.total = 2;
    render(<UpcomingView />);
    const section = screen.getByRole("region", {
      name: /week|Week/,
    });
    const cards = section.querySelectorAll("[data-card]");
    expect(cards).toHaveLength(2);
    expect(cards[0]?.querySelector("[data-stage]")).toBeNull();
    expect(cards[1]?.querySelector("[data-stage]")?.textContent).toBe(
      "Tracked",
    );
    expect(screen.getByTestId("upcoming-total").textContent).toBe("2 releases");
  });

  it("opens a read-only dialog: links out, nothing that acts on Mylar", () => {
    state.weeks = [
      week([
        entry({
          tracked: true,
          mylar_status: "wanted",
          stage: "wanted",
          library_series: { slug: "action-comics", name: "Action Comics" },
        }),
      ]),
    ];
    render(<UpcomingView />);
    fireEvent.click(document.querySelector("[data-card]") as HTMLElement);
    const dialog = within(screen.getByRole("dialog"));
    expect(dialog.getByText("Action Comics #1104")).toBeTruthy();
    expect(dialog.getByText("Mylar is looking for it.")).toBeTruthy();
    expect(
      dialog.getByRole("link", { name: "Open series" }).getAttribute("href"),
    ).toBe("/series/action-comics");
    expect(
      dialog.getByRole("link", { name: /View on Metron/ }).getAttribute("rel"),
    ).toContain("noreferrer");
    // The only button is the dialog's own close control.
    expect(dialog.getAllByRole("button").map((b) => b.textContent)).toEqual([
      "Close",
    ]);
  });

  it("offers Add series to Mylar only when allowed, known and untracked", () => {
    const untracked = entry({ cv_series_id: 4242 });
    const open = () => {
      fireEvent.click(document.querySelector("[data-card]") as HTMLElement);
      return within(screen.getByRole("dialog"));
    };
    const offered = (d: ReturnType<typeof open>) =>
      d.queryByRole("button", { name: "Add series to Mylar" }) !== null;

    // Not allowed (not an admin, or no Mylar address and key stored).
    state.weeks = [week([untracked])];
    const first = render(<UpcomingView />);
    expect(offered(open())).toBe(false);
    first.unmount();

    state.summary = summary({ can_add_to_mylar: true });
    // Already tracked, or no ComicVine id to send.
    for (const e of [
      entry({ cv_series_id: 4242, tracked: true, stage: "tracked" }),
      entry({ cv_series_id: null }),
    ]) {
      state.weeks = [week([e])];
      const r = render(<UpcomingView />);
      expect(offered(open())).toBe(false);
      r.unmount();
    }

    state.weeks = [week([untracked])];
    render(<UpcomingView />);
    const dialog = open();
    fireEvent.click(
      dialog.getByRole("button", { name: "Add series to Mylar" }),
    );
    expect(addSeries.mock.calls[0]![0]).toEqual({
      cv_series_id: 4242,
      series_name: "Action Comics",
    });
  });

  it("renders the same releases as rows in the list layout", () => {
    window.localStorage.setItem(UPCOMING_LAYOUT_KEY, "list");
    state.weeks = [week([entry({ stage: "in_library", owned: true })])];
    render(<UpcomingView />);
    const table = screen.getByRole("table");
    expect(
      within(table).getByRole("button", { name: "Action Comics #1104" }),
    ).toBeTruthy();
    expect(within(table).getByText("In library")).toBeTruthy();
    expect(document.querySelector("[data-card]")).toBeNull();
  });
});

describe("UpcomingView empty states", () => {
  it("says the feature is off, with the settings link for admins only", () => {
    state.summary = summary({ enabled: false, mylar: null, publishers: [] });
    const { unmount } = render(<UpcomingView />);
    expect(screen.getByText("Upcoming releases are turned off")).toBeTruthy();
    expect(
      screen
        .getByRole("link", { name: "Open Upcoming settings" })
        .getAttribute("href"),
    ).toBe("/admin/upcoming");
    expect(document.querySelector("[data-toolbar-row]")).toBeNull();
    unmount();

    state.role = "user";
    render(<UpcomingView />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("offers all releases when nothing in the reader's series is coming", () => {
    render(<UpcomingView />);
    expect(screen.getByText("Nothing upcoming in your series")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show all releases" }));
    expect(lastFilters().scope).toBe("all");
  });

  it("clears filters from the no-match state", () => {
    render(<UpcomingView />);
    fireEvent.click(toolbar().getByRole("button", { name: "New #1s" }));
    expect(screen.getByText("No releases match these filters")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(lastFilters().firstIssues).toBeUndefined();
  });

  it("explains an empty calendar that has never been fetched", () => {
    state.summary = summary({ calendar_synced_at: null, mylar: null });
    render(<UpcomingView />);
    expect(
      screen.getByText("The release calendar is not filled yet"),
    ).toBeTruthy();
  });

  it("blames the filter, not the unfetched calendar, when a filter is on", () => {
    // Mylar's pull list can fill the page before the calendar is fetched;
    // a filter that hides those rows is the reason the list is empty.
    state.summary = summary({ calendar_synced_at: null });
    render(<UpcomingView />);
    fireEvent.click(toolbar().getByRole("button", { name: "New #1s" }));
    expect(screen.getByText("No releases match these filters")).toBeTruthy();
    expect(
      screen.queryByText("The release calendar is not filled yet"),
    ).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(
      screen.getByText("The release calendar is not filled yet"),
    ).toBeTruthy();
  });
});
