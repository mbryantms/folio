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
  ];
  state.lists = [
    list("l1", { total: 12, matched: 10, read_count: 2, missing: 2 }),
    list("l2", { total: 5, matched: 5, read_count: 3 }),
    list("l3", { total: 4, matched: 4, read_count: 4 }),
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
  it("draws one column header for the whole table", async () => {
    const { container } = renderIndex();
    await rowOf("Crisis order");
    const headers = container.querySelectorAll("[data-board-header]");
    expect(headers).toHaveLength(1);
    const header = headers[0] as HTMLElement;
    expect(within(header).getByRole("button", { name: "Name" })).toBeTruthy();
    expect(within(header).getByRole("button", { name: "Read" })).toBeTruthy();
    expect(within(header).getByText("In library")).toBeTruthy();
    expect(
      within(header).getByRole("button", { name: "Updated" }),
    ).toBeTruthy();
    // The group is a folder row inside the same table, not a second header.
    expect(screen.getByRole("region", { name: "Group Events" })).toBeTruthy();
  });

  it("rolls the folder's lists up into its row", async () => {
    renderIndex();
    await rowOf("Crisis order");
    const folder = screen.getByRole("region", { name: "Group Events" });
    const folderRow = folder.firstElementChild as HTMLElement;
    // Read 2 + 3 of matched 10 + 5; in library 15 of 12 + 5.
    expect(within(folderRow).getByText("5 of 15")).toBeTruthy();
    expect(within(folderRow).getByText("15 of 17")).toBeTruthy();
    expect(within(folderRow).getByText("2 lists")).toBeTruthy();
    expect(
      within(folderRow).getByRole("button", {
        name: "Actions for group Events",
      }),
    ).toBeTruthy();
  });

  it("shows a list's name, description, numbers and date", async () => {
    renderIndex();
    const row = await rowOf("Crisis order");
    expect(within(row).getByRole("link").getAttribute("href")).toBe(
      "/views/v1",
    );
    expect(
      within(row).getByText("Every issue of the 1985 event in order"),
    ).toBeTruthy();
    expect(within(row).getByText("2 of 10")).toBeTruthy(); // Read
    expect(within(row).getByText("10 of 12")).toBeTruthy(); // In library
    const updated = within(row)
      .getAllByText("3 days ago")
      .find((el) => el.closest("[title]")?.getAttribute("title"));
    expect(updated?.closest("[title]")?.getAttribute("title")).toMatch(
      /^Last refreshed /,
    );
    expect(
      within(row).getByRole("img", { name: "Pinned to 2 pages" }),
    ).toBeTruthy();
    expect(
      within(row).getByRole("img", { name: "In the sidebar" }),
    ).toBeTruthy();
    // The narrow-width caption folds the columns together.
    expect(row.querySelector("[data-row-caption]")?.textContent).toContain(
      "2 of 10 read · 2 missing",
    );
  });

  it("calls the missing count out only when there is one", async () => {
    renderIndex();
    const crisis = await rowOf("Crisis order");
    const missing = crisis.querySelector("[data-row-missing]");
    expect(missing?.textContent).toBe("2 missing");
    expect(missing?.className).toContain("text-warning");
    const watchmen = await rowOf("Watchmen");
    expect(watchmen.querySelector("[data-row-missing]")).toBeNull();
    expect(within(watchmen).queryByText(/missing/)).toBeNull();
  });

  it("switches density from the toolbar and remembers it", async () => {
    const first = renderIndex();
    let row = await rowOf("Crisis order");
    expect(row.getAttribute("data-density")).toBe("compact");
    const link = within(row).getByRole("link");
    expect(link.className).toContain("lg:flex-row");

    fireEvent.click(screen.getByRole("radio", { name: "Comfortable rows" }));
    row = await rowOf("Crisis order");
    await waitFor(() =>
      expect(row.getAttribute("data-density")).toBe("comfortable"),
    );
    expect(within(row).getByRole("link").className).not.toContain(
      "lg:flex-row",
    );
    expect(window.localStorage.getItem("folio.readingLists.density")).toBe(
      "comfortable",
    );

    first.unmount();
    renderIndex();
    row = await rowOf("Crisis order");
    expect(row.getAttribute("data-density")).toBe("comfortable");
  });

  it("never clamps a long name at desktop; the description gives way", async () => {
    renderIndex();
    const row = await rowOf(LONG_NAME);
    const name = row.querySelector("[data-row-name]") as HTMLElement;
    const desc = row.querySelector("[data-row-description]") as HTMLElement;
    expect(name.textContent).toBe(LONG_NAME);
    expect(name.getAttribute("title")).toBe(LONG_NAME);
    const classes = (el: HTMLElement) => el.className.split(/\s+/);
    // No line clamp from lg up (phones get two lines).
    expect(
      classes(name).filter(
        (c) => /line-clamp/.test(c) && !c.startsWith("max-sm:"),
      ),
    ).toEqual([]);
    // Compact: the name keeps its width (only the column can cut it), the
    // description takes what is left.
    expect(classes(name)).toEqual(
      expect.arrayContaining(["lg:shrink-0", "lg:max-w-full"]),
    );
    expect(classes(desc)).toEqual(
      expect.arrayContaining(["lg:flex-1", "lg:min-w-0", "lg:truncate"]),
    );

    fireEvent.click(screen.getByRole("radio", { name: "Comfortable rows" }));
    await waitFor(() =>
      expect(row.getAttribute("data-density")).toBe("comfortable"),
    );
    // Comfortable: the name wraps rather than truncating.
    expect(
      classes(name).filter(
        (c) => /truncate|line-clamp/.test(c) && !c.startsWith("max-sm:"),
      ),
    ).toEqual([]);
  });

  it("keeps every item in a list's ⋯ menu", async () => {
    renderIndex();
    const row = await rowOf("Crisis order");
    const trigger = within(row).getByRole("button", {
      name: "Actions for Crisis order",
    });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    expect(
      await screen.findByRole("menuitem", { name: /Pin to pages/ }),
    ).toBeTruthy();
    expect(
      screen.getByRole("menuitemcheckbox", { name: /Show in sidebar/ }),
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
