// @vitest-environment jsdom
/**
 * The reading lists as `GroupedViewsBoard`'s rows presentation (the tree
 * table): one column header, folder rows with roll-ups over their lists,
 * a row per list with name, description, Read / In library / Updated,
 * the missing count called out only when there is one, the density
 * toggle (remembered per browser), names that are not clamped at
 * desktop, and the ⋯ menu with every item. The card presentation the
 * collections and filter views use is unchanged. The real query hooks
 * run against a mocked `apiFetch`.
 */
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Folder } from "lucide-react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({
  views: [] as unknown[],
  lists: [] as unknown[],
  groups: [] as unknown[],
}));

function json(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => "csrf",
  apiFetch: async (path: string) => {
    if (path.startsWith("/auth/me")) return json({ id: "u1", role: "user" });
    if (path.startsWith("/me/saved-views")) return json({ items: state.views });
    if (path.startsWith("/me/cbl-lists")) return json({ items: state.lists });
    if (path.startsWith("/me/view-groups"))
      return json({ scope: "cbl", groups: state.groups, ungrouped_ids: [] });
    return json({ items: [] });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/lists",
  useSearchParams: () => new URLSearchParams(),
}));

import { ReadingListsIndex } from "@/components/cbl/ReadingListsIndex";
import { readingListRows } from "@/components/cbl/ReadingListRows";
import { GroupedViewsBoard } from "@/components/saved-views/GroupedViewsBoard";
import { ViewCard } from "@/components/saved-views/ViewsIndex";

const DAY = 24 * 60 * 60 * 1000;

function view(over: Record<string, unknown>) {
  return {
    id: "v",
    user_id: "u1",
    kind: "cbl",
    name: "List",
    description: null,
    custom_year_start: null,
    custom_year_end: null,
    custom_tags: [],
    match_mode: null,
    conditions: null,
    sort_field: null,
    sort_order: null,
    result_limit: null,
    cbl_list_id: null,
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    pinned: false,
    pinned_position: null,
    show_in_sidebar: false,
    is_system: false,
    system_key: null,
    icon: null,
    pinned_on_pages: [],
    ...over,
  };
}

function list(id: string, stats: Record<string, number>, refreshedDaysAgo = 3) {
  return {
    id,
    source_kind: "upload",
    parsed_name: id,
    parsed_matchers_present: true,
    imported_at: "2025-01-01T00:00:00Z",
    created_at: "2025-01-01T00:00:00Z",
    updated_at: "2025-01-01T00:00:00Z",
    last_refreshed_at: new Date(
      Date.now() - refreshedDaysAgo * DAY,
    ).toISOString(),
    owner_user_id: "u1",
    stats: {
      total: 0,
      matched: 0,
      read_count: 0,
      ambiguous: 0,
      missing: 0,
      manual: 0,
      ...stats,
    },
  };
}

const LONG_NAME =
  "Crisis on Infinite Earths: the complete reading order including every tie-in, prelude and epilogue";

function seed() {
  state.views = [
    view({
      id: "v1",
      name: "Crisis order",
      description: "Every issue of the 1985 event in order",
      cbl_list_id: "l1",
      pinned_on_pages: ["p1", "p2"],
      show_in_sidebar: true,
    }),
    view({ id: "v2", name: "Watchmen", cbl_list_id: "l2" }),
    view({
      id: "v3",
      name: LONG_NAME,
      description: "A description that is long enough to be cut first",
      cbl_list_id: "l3",
    }),
    view({ id: "v4", name: "Wishlist", cbl_list_id: "l4" }),
  ];
  state.lists = [
    list("l1", {
      total: 13,
      matched: 10,
      read_count: 2,
      missing: 2,
      ambiguous: 1,
    }),
    list("l2", { total: 5, matched: 5, read_count: 3 }),
    list("l3", { total: 4, matched: 4, read_count: 4 }),
    list("l4", { total: 6, missing: 6 }),
  ];
  state.groups = [
    {
      id: "g1",
      scope: "cbl",
      name: "Events",
      position: 0,
      collapsed: false,
      view_ids: ["v1", "v2"],
    },
  ];
}

function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

function renderIndex() {
  return render(
    <QueryClientProvider client={client()}>
      <ReadingListsIndex />
    </QueryClientProvider>,
  );
}

/** The `<li>` row of the list whose name is `name`. */
async function rowOf(name: string) {
  const el = await screen.findByText(name, { selector: "[data-row-name]" });
  return el.closest("li") as HTMLElement;
}

beforeEach(() => {
  cleanup();
  window.localStorage.clear();
  seed();
});

describe("reading lists tree table", () => {
  it("draws one column header for the whole table, with no date column", async () => {
    const { container } = renderIndex();
    await rowOf("Crisis order");
    const headers = container.querySelectorAll("[data-board-header]");
    expect(headers).toHaveLength(1);
    const header = headers[0] as HTMLElement;
    expect(within(header).getByRole("button", { name: "Name" })).toBeTruthy();
    expect(within(header).getByRole("button", { name: "Read" })).toBeTruthy();
    expect(within(header).getByText("In library")).toBeTruthy();
    expect(within(header).queryByText(/Updated/)).toBeNull();
    // The group is a folder row inside the same table, not a second header.
    expect(screen.getByRole("region", { name: "Group Events" })).toBeTruthy();
  });

  it("rolls the folder's lists up into its row", async () => {
    renderIndex();
    await rowOf("Crisis order");
    const folder = screen.getByRole("region", { name: "Group Events" });
    const folderRow = folder.firstElementChild as HTMLElement;
    // Read 2 + 3 of matched 10 + 5; in library 15 of 13 + 5.
    expect(within(folderRow).getByText("5 of 15")).toBeTruthy();
    expect(folderRow.textContent).toContain(
      "15 of 18 · 2 missing · 1 to match",
    );
    expect(within(folderRow).getByText("2 lists")).toBeTruthy();
    expect(
      within(folderRow).getByRole("button", {
        name: "Actions for group Events",
      }),
    ).toBeTruthy();
    // The folder name toggles its rows and says so.
    expect(
      within(folderRow)
        .getByRole("button", { name: "Events" })
        .getAttribute("aria-expanded"),
    ).toBe("true");
  });

  it("shows a list's name, flags, description and numbers, its date as a tooltip", async () => {
    renderIndex();
    const row = await rowOf("Crisis order");
    expect(within(row).getByRole("link").getAttribute("href")).toBe(
      "/views/v1",
    );
    expect(
      within(row).getByText("Every issue of the 1985 event in order"),
    ).toBeTruthy();
    expect(within(row).getByText("2 of 10")).toBeTruthy(); // Read
    const library = row.querySelector("[data-row-gap]")!.parentElement!;
    expect(library.textContent).toBe(
      "In library: 10 of 13 · 2 missing · 1 to match",
    );
    expect(library.getAttribute("title")).toMatch(/^Updated 3 days ago \(/);
    // Flags sit right after the name, only where they apply.
    const name = row.querySelector("[data-row-name]") as HTMLElement;
    expect(
      within(name).getByRole("img", { name: "Pinned to 2 pages" }),
    ).toBeTruthy();
    expect(
      within(name).getByRole("img", { name: "In the sidebar" }),
    ).toBeTruthy();
    const watchmen = await rowOf("Watchmen");
    const watchmenName = watchmen.querySelector(
      "[data-row-name]",
    ) as HTMLElement;
    expect(within(watchmenName).queryAllByRole("img")).toEqual([]);
    // Below the wide tier the In library figures fold into the caption.
    expect(row.querySelector("[data-row-caption]")?.textContent).toBe(
      "2 of 10 read · 2 missing · 1 to match",
    );
  });

  it("marks a list whose refresh is failing, and only that one", async () => {
    const since = "2026-11-03T09:00:00Z";
    (state.lists[1] as Record<string, unknown>).refresh_failure = {
      kind: "unreachable",
      detail: null,
      failing_since: since,
      consecutive_failures: 4,
    };
    renderIndex();
    const watchmen = await rowOf("Watchmen");
    const name = watchmen.querySelector("[data-row-name]") as HTMLElement;
    const day = new Date(since).toLocaleDateString(undefined, {
      day: "numeric",
      month: "short",
      ...(new Date(since).getFullYear() === new Date().getFullYear()
        ? {}
        : { year: "numeric" }),
    });
    const label = `Refresh failing since ${day}: the source could not be reached`;
    const marker = within(name).getByRole("img", { name: label });
    expect(marker.hasAttribute("data-row-refresh-failing")).toBe(true);
    // The In library tooltip does not claim a fresh update.
    const library = watchmen.querySelector("[data-row-caption]")!;
    const title = library.getAttribute("title") ?? "";
    expect(title).toMatch(/^Last updated /);
    expect(title).toContain(`Refresh failing since ${day}`);
    // A healthy list shows nothing new.
    const crisis = await rowOf("Crisis order");
    expect(crisis.querySelector("[data-row-refresh-failing]")).toBeNull();
    expect(
      within(crisis).queryByRole("img", { name: /Refresh failing/ }),
    ).toBeNull();
    expect(
      crisis.querySelector("[data-row-caption]")!.getAttribute("title"),
    ).toMatch(/^Updated /);
  });

  it("calls the missing count out only when there is one", async () => {
    renderIndex();
    const crisis = await rowOf("Crisis order");
    const gaps = [...crisis.querySelectorAll("[data-row-gap]")];
    expect(gaps.map((g) => g.textContent)).toEqual(["2 missing", "1 to match"]);
    expect(gaps[0]!.className).toContain("text-warning");
    const watchmen = await rowOf("Watchmen");
    expect(watchmen.querySelector("[data-row-gap]")).toBeNull();
    expect(within(watchmen).queryByText(/missing/)).toBeNull();
  });

  it("says there is nothing to read rather than 0 of 0", async () => {
    renderIndex();
    const row = await rowOf("Wishlist");
    // The Read figure (wide) and the caption (narrower) both say it.
    expect(within(row).getAllByText("Nothing to read yet")).toHaveLength(2);
    expect(within(row).queryByText("0 of 0")).toBeNull();
    // The hatched bar still shows what is missing.
    expect(
      within(row).getByRole("img", { name: "6 not in your library" }),
    ).toBeTruthy();
  });

  it("defaults to comfortable, switches density from the toolbar and remembers it", async () => {
    const first = renderIndex();
    let row = await rowOf("Crisis order");
    expect(row.getAttribute("data-density")).toBe("comfortable");

    fireEvent.click(screen.getByRole("radio", { name: "Compact rows" }));
    row = await rowOf("Crisis order");
    await waitFor(() =>
      expect(row.getAttribute("data-density")).toBe("compact"),
    );
    expect(window.localStorage.getItem("folio.readingLists.density")).toBe(
      "compact",
    );

    first.unmount();
    renderIndex();
    row = await rowOf("Crisis order");
    expect(row.getAttribute("data-density")).toBe("compact");
  });

  it("never cuts a name to one line on a wide table, in either density", async () => {
    renderIndex();
    const row = await rowOf(LONG_NAME);
    const link = within(row).getByRole("link");
    const name = row.querySelector("[data-row-name]") as HTMLElement;
    const desc = row.querySelector("[data-row-description]") as HTMLElement;
    const classes = (el: HTMLElement) => el.className.split(/\s+/);
    const wide = (el: HTMLElement) =>
      classes(el).filter((c) => c.startsWith("@min-[42rem]/board:"));
    expect(name.textContent).toBe(LONG_NAME);
    expect(name.getAttribute("title")).toBe(LONG_NAME);
    // No rule anywhere truncates the name to a single line.
    expect(
      classes(name).some((c) => /truncate|whitespace-nowrap/.test(c)),
    ).toBe(false);

    // Comfortable (default): from the medium tier up the name is never
    // clamped (narrow tables clamp at two lines); the description gets
    // two lines under it.
    expect(classes(name)).toEqual(
      expect.arrayContaining([
        "line-clamp-2",
        "@min-[30rem]/board:line-clamp-none",
      ]),
    );
    expect(wide(name)).toEqual([]);
    expect(classes(desc)).toContain("line-clamp-2");
    expect(wide(link)).toEqual([]);

    // Compact: name and description share a wrapping line, two lines tall
    // at most; a name that does not fit wraps to its second line (full
    // name in the title) and pushes the description out of the box; a
    // description that does fit is cut with an ellipsis.
    fireEvent.click(screen.getByRole("radio", { name: "Compact rows" }));
    await waitFor(() =>
      expect(row.getAttribute("data-density")).toBe("compact"),
    );
    expect(wide(link)).toEqual(
      expect.arrayContaining([
        "@min-[42rem]/board:flex-row",
        "@min-[42rem]/board:flex-wrap",
        "@min-[42rem]/board:max-h-10",
        "@min-[42rem]/board:overflow-hidden",
      ]),
    );
    expect(wide(name)).toContain("@min-[42rem]/board:line-clamp-2");
    expect(wide(desc)).toEqual(
      expect.arrayContaining([
        "@min-[42rem]/board:basis-0",
        "@min-[42rem]/board:min-w-24",
        "@min-[42rem]/board:flex-1",
        "@min-[42rem]/board:truncate",
      ]),
    );
  });

  it("sorts inside every folder from the header and keeps folders in place", async () => {
    renderIndex();
    await rowOf("Crisis order");
    const order = () =>
      [...document.querySelectorAll("[data-board-row] [data-row-name]")].map(
        (n) => n.textContent,
      );
    // Custom order: Events holds Crisis (2 of 10) then Watchmen (3 of 5).
    expect(order().slice(0, 2)).toEqual(["Crisis order", "Watchmen"]);
    const header = document.querySelector("[data-board-header]") as HTMLElement;
    const read = within(header).getByRole("button", { name: "Read" });
    fireEvent.click(read);
    await waitFor(() =>
      expect(order().slice(0, 2)).toEqual(["Watchmen", "Crisis order"]),
    );
    expect(read.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("combobox", { name: "Sort" }).textContent).toBe(
      "Most read",
    );
    // The folder row is still first; the grip says what a drag does now.
    const folder = screen.getByRole("region", { name: "Group Events" });
    expect(folder.compareDocumentPosition(await rowOf(LONG_NAME))).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    const grip = within(await rowOf("Watchmen")).getByRole("button", {
      name: "Drag Watchmen",
    });
    expect(grip.getAttribute("title")).toMatch(/^Sorted by most read/);
    // Pressing it again returns to the hand-arranged order.
    fireEvent.click(read);
    await waitFor(() =>
      expect(order().slice(0, 2)).toEqual(["Crisis order", "Watchmen"]),
    );
  });

  it("keeps every item in a list's ⋯ menu, saying where it is pinned", async () => {
    renderIndex();
    const row = await rowOf("Crisis order");
    const trigger = within(row).getByRole("button", {
      name: "Actions for Crisis order",
    });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    expect(
      await screen.findByRole("menuitem", { name: /Pinned to 2 pages/ }),
    ).toBeTruthy();
    expect(
      screen.getByRole("menuitem", { name: /Hide from sidebar/ }),
    ).toBeTruthy();
    expect(
      screen.getByRole("menuitem", { name: /Move to group/ }),
    ).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /Delete/ })).toBeTruthy();
  });
});

describe("card presentation", () => {
  it("collections still render as cards, with no table header", async () => {
    state.views = [];
    state.groups = [];
    const collections = [
      view({ id: "c1", kind: "collection", name: "Favourites" }),
    ] as never[];
    const { container } = render(
      <QueryClientProvider client={client()}>
        <GroupedViewsBoard
          scope="collection"
          views={collections}
          loading={false}
          blurb="Hand-picked series and issues."
          noun="collection"
          icon={Folder}
          action={null}
          empty="none"
          renderCard={(v) => <ViewCard view={v} Icon={Folder} isCollection />}
        />
      </QueryClientProvider>,
    );
    const name = await screen.findByText("Favourites");
    expect(name.closest("[data-board-card]")).toBeTruthy();
    expect(container.querySelector("[data-board-row]")).toBeNull();
    expect(container.querySelector("[data-board-header]")).toBeNull();
    expect(screen.queryByRole("radio", { name: "Compact rows" })).toBeNull();
  });
});

describe("reading-list sorts", () => {
  it('"Recently updated" orders by the list\'s date, replacing the view-row sort', () => {
    const lists = new Map(
      [list("old", {}, 30), list("new", {}, 1)].map((l) => [l.id, l as never]),
    );
    const rows = readingListRows(lists);
    expect(rows.hiddenSorts).toEqual(["updated"]);
    const refreshed = rows.sorts!.find((x) => x.value === "board:refreshed")!;
    expect(refreshed.label).toBe("Recently updated");
    // The view rows say the opposite (`updated_at`): the list date wins.
    const a = view({
      id: "a",
      name: "A",
      cbl_list_id: "old",
      updated_at: "2026-11-10T00:00:00Z",
    }) as never;
    const b = view({
      id: "b",
      name: "B",
      cbl_list_id: "new",
      updated_at: "2020-01-01T00:00:00Z",
    }) as never;
    expect(
      [a, b].sort(refreshed.compare).map((x: { id: string }) => x.id),
    ).toEqual(["b", "a"]);
  });
});
