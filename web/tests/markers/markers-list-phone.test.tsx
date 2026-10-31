// @vitest-environment jsdom
/**
 * `/bookmarks` phone chrome: below `sm` the header toolbar, the layout
 * toggle and the Tags row give way to a `PhoneControlsBar` whose
 * Filters button opens a bottom sheet (layout + tags) and whose `⋯`
 * sheet carries card size + Select + the two export links. jsdom has no
 * CSS, so both compositions render here — the assertions pin the
 * *wiring* (what each sheet contains, the filter badge count, the
 * `max-sm:` hooks) rather than visibility.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const infinite = vi.fn();
let tags: { tag: string; count: number }[] = [];

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
}));

vi.mock("@/lib/api/queries", () => ({
  useBookmarksFeedInfinite: (filters: unknown) => {
    infinite(filters);
    return {
      data: { pages: [{ items: [], next_cursor: null }] },
      isLoading: false,
      isError: false,
      hasNextPage: false,
      isFetchingNextPage: false,
      fetchNextPage: vi.fn(),
    };
  },
  useBookmarkCounts: () => ({
    data: {
      total: 0,
      all: 0,
      by_kind: {
        bookmark: 0,
        note: 0,
        favorite: 0,
        highlight: 0,
        closing_note: 0,
      },
    },
  }),
  useBookmarkSeries: () => ({ data: { items: [] } }),
  useMarkerTags: () => ({ data: { items: tags } }),
}));

vi.mock("@/lib/api/mutations", () => {
  const m = () => ({ mutate: vi.fn(), isPending: false });
  return {
    useBulkDeleteMarkers: m,
    useCreateMarker: m,
    useDeleteMarker: m,
    useRestoreMarkers: m,
    useUpdateMarker: m,
  };
});

const { MarkersList } = await import("@/components/markers/MarkersList");

describe("MarkersList phone chrome", () => {
  beforeEach(() => {
    infinite.mockClear();
    tags = [
      { tag: "art", count: 3 },
      { tag: "plot", count: 1 },
    ];
  });

  it("renders the phone bar on /bookmarks and hides the desktop toolbar below sm", () => {
    render(<MarkersList />);
    const bar = document.querySelector("[data-phone-controls]");
    expect(bar).not.toBeNull();
    expect(
      within(bar as HTMLElement).getByRole("button", {
        name: "Filter saved markers",
      }),
    ).toBeTruthy();
    expect(
      within(bar as HTMLElement).getByRole("button", { name: "Filters" }),
    ).toBeTruthy();
    expect(
      within(bar as HTMLElement).getByRole("button", { name: "View options" }),
    ).toBeTruthy();
    // The title rides in the bar's leading slot so title + controls
    // share one sticky row on phones; the desktop PageHeader (whose
    // own h1 is the other "Bookmarks" heading) carries the phone-hide
    // hook as a whole, not just its toolbar.
    expect(
      within(bar as HTMLElement).getByRole("heading", {
        level: 1,
        name: "Bookmarks",
      }),
    ).toBeTruthy();
    const desktopHeader = screen
      .getAllByRole("heading", { level: 1, name: "Bookmarks" })
      .map((h) => h.closest("header"))
      .find((h): h is HTMLElement => h !== null);
    expect(desktopHeader?.className).toContain("max-sm:hidden");
    // Desktop header toolbar is still rendered (desktop) but carries
    // the phone-hide hook; so do the inline layout toggle + Tags row.
    // Both widths render a `⋯` (export): the desktop one sits in the
    // sticky row's right cluster, which carries the phone-hide hook.
    const desktopMore = screen
      .getAllByRole("button", { name: "More actions" })
      .find((b) => !(bar as HTMLElement).contains(b));
    expect(desktopMore?.closest("[data-toolbar-row]")?.className).toContain(
      "max-sm:hidden",
    );
    const inlineLayout = screen
      .getAllByRole("group", { name: "Marker layout" })
      .find((g) => g.className.includes("ml-auto"));
    expect(inlineLayout?.className).toContain("max-sm:hidden");
    const tablist = screen.getByRole("tablist", { name: "Filter by kind" });
    expect(tablist.className).toContain("max-sm:overflow-x-auto");
    expect(tablist.className).toContain("max-sm:flex-nowrap");
  });

  it("does not render the phone bar when embedded in a series/issue tab", () => {
    render(<MarkersList scope={{ kind: "series", seriesId: "s-1" }} />);
    expect(document.querySelector("[data-phone-controls]")).toBeNull();
    // Embedded keeps the inline Tags row visible at every width.
    expect(screen.getByText("Tags").parentElement?.className).not.toContain(
      "max-sm:hidden",
    );
  });

  it("Filters sheet holds the layout toggle + tags and the badge counts both", () => {
    render(<MarkersList />);
    const bar = document.querySelector("[data-phone-controls]") as HTMLElement;
    fireEvent.click(within(bar).getByRole("button", { name: "Filters" }));
    const sheet = screen.getByRole("dialog", { name: "Filters" });
    // Layout toggle inside the sheet.
    const layout = within(sheet).getByRole("group", { name: "Marker layout" });
    fireEvent.click(within(layout).getByRole("button", { name: "Newest" }));
    // Tag pill inside the sheet drives the server-side filter.
    fireEvent.click(within(sheet).getByRole("button", { name: /art/ }));
    expect(infinite).toHaveBeenLastCalledWith(
      expect.objectContaining({ tags: "art", tag_match: "all" }),
    );
    // Second tag → the Match all/any toggle appears.
    fireEvent.click(within(sheet).getByRole("button", { name: /plot/ }));
    expect(
      within(sheet).getByRole("group", { name: "Tag match mode" }),
    ).toBeTruthy();
    // Badge = 2 tags + 1 for the non-default layout. (The modal sheet
    // marks the page `aria-hidden`, so read the bar's label directly.)
    const filtersLabel = () =>
      bar
        .querySelector('button[aria-label^="Filters"]')
        ?.getAttribute("aria-label");
    expect(filtersLabel()).toBe("Filters (3 active)");
    // Clear tags drops the tag part of the count.
    fireEvent.click(within(sheet).getByRole("button", { name: "Clear tags" }));
    expect(filtersLabel()).toBe("Filters (1 active)");
    fireEvent.click(within(sheet).getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog", { name: "Filters" })).toBeNull();
  });

  it("View options holds card size; export is behind the bar's ⋯ (rule 11)", () => {
    render(<MarkersList />);
    const bar = document.querySelector("[data-phone-controls]") as HTMLElement;
    // ⋯ is the last control in the bar, after View options.
    const buttons = within(bar)
      .getAllByRole("button")
      .map((b) => b.getAttribute("aria-label"));
    expect(buttons.slice(-2)).toEqual(["View options", "More actions"]);
    fireEvent.click(within(bar).getByRole("button", { name: "View options" }));
    const sheet = screen.getByRole("dialog", { name: "View options" });
    expect(within(sheet).getByText("Card size")).toBeTruthy();
    expect(within(sheet).getByRole("slider")).toBeTruthy();
    expect(within(sheet).queryByRole("link", { name: /export/i })).toBeNull();
    // No markers loaded → no Select row.
    expect(within(sheet).queryByRole("button", { name: "Select" })).toBeNull();
    fireEvent.keyDown(within(sheet).getByText("Card size"), { key: "Escape" });
    const more = within(bar).getByRole("button", { name: "More actions" });
    fireEvent.keyDown(more, { key: "Enter" });
    const md = screen.getByRole("menuitem", {
      name: /export notes as markdown/i,
    });
    const json = screen.getByRole("menuitem", {
      name: /export notes as json/i,
    });
    expect(md.getAttribute("href")).toContain("format=md");
    expect(json.getAttribute("href")).toContain("format=json");
    expect(md.hasAttribute("download")).toBe(true);
  });
});
