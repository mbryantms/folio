// @vitest-environment jsdom
/**
 * What the board sends while a sort is on, through the real component:
 * the layout PUT after "Move to group" and the layout Undo restores
 * after a group delete carry the stored, hand-arranged order (a moved
 * list appended to its target), never the sorted order on screen. Also:
 * "Recently updated" in the sort menu orders rows by the list's own
 * date. With nested groups: moving a list or a folder by menu, deleting
 * a nested folder and its Undo, all also while sorted, and the depth cap
 * disabling menu items. The query hooks run against a stateful mocked
 * `apiFetch` that mirrors the server's tree rules.
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
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as CoarsePointer from "@/lib/ui/use-coarse-pointer";

type Group = {
  id: string;
  scope: string;
  name: string;
  parent_id: string | null;
  position: number;
  collapsed: boolean;
  view_ids: string[];
};

const state = vi.hoisted(() => ({
  views: [] as unknown[],
  lists: [] as unknown[],
  groups: [] as Group[],
  ungrouped: [] as string[],
  calls: [] as Array<{ path: string; method: string; body: unknown }>,
  undo: [] as Array<() => unknown>,
  /** Refuse the next layout PUT with this 422 message. */
  refusePut: null as string | null,
  /** Fail the next request matching `method path`: with a status (the
   *  server refused or broke, nothing applied), or "lost" (the server
   *  applied it and the answer never came back). */
  fail: [] as Array<{ call: string; status: number | "lost" }>,
  /** The options of every Undo toast raised. */
  undoOpts: [] as Array<{
    restoredMessage?: string | null | ((r: never) => string | null);
  }>,
  toastError: vi.fn(),
  toastInfo: vi.fn(),
}));

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => "csrf",
  apiFetch: async (path: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    if (method !== "GET") state.calls.push({ path, method, body });
    const failAt = state.fail.findIndex((f) => f.call === `${method} ${path}`);
    const fail = failAt < 0 ? null : state.fail.splice(failAt, 1)[0]!;
    if (fail && fail.status !== "lost")
      return json(
        { error: { code: "test", message: `failed ${fail.status}` } },
        fail.status,
      );
    const lost = fail !== null;
    const answer = (r: Response) => {
      if (lost) throw new TypeError("Failed to fetch");
      return r;
    };
    if (path.startsWith("/auth/me")) return json({ id: "u1", role: "user" });
    if (path.startsWith("/me/saved-views")) return json({ items: state.views });
    if (path.startsWith("/me/cbl-lists")) return json({ items: state.lists });
    if (path === "/me/view-groups" && method === "POST") {
      const parent = body.parent_id ?? null;
      const g: Group = {
        id: "g-new",
        scope: "cbl",
        name: body.name,
        parent_id: parent,
        position: state.groups.filter((x) => x.parent_id === parent).length,
        collapsed: false,
        view_ids: body.view_ids,
      };
      state.groups = [...state.groups, g];
      return json(g, 201);
    }
    if (path.startsWith("/me/view-groups/") && method === "DELETE") {
      serverDelete(path.split("/").pop()!);
      return new Response(null, { status: 204 });
    }
    if (path === "/me/view-groups/layout" && method === "PUT") {
      if (state.refusePut) {
        const message = state.refusePut;
        state.refusePut = null;
        return json({ error: { code: "validation", message } }, 422);
      }
      const byId = new Map(state.groups.map((g) => [g.id, g]));
      const next = new Map<string | null, number>();
      state.groups = body.groups.map(
        (g: { id: string; parent_id?: string | null; view_ids: string[] }) => {
          const parent = g.parent_id ?? null;
          const position = next.get(parent) ?? 0;
          next.set(parent, position + 1);
          return {
            ...byId.get(g.id)!,
            parent_id: parent,
            position,
            view_ids: g.view_ids,
          };
        },
      );
      state.ungrouped = body.ungrouped;
      return answer(new Response(null, { status: 204 }));
    }
    if (method !== "GET") return new Response(null, { status: 204 });
    if (path.startsWith("/me/view-groups"))
      return json({
        scope: "cbl",
        groups: state.groups,
        ungrouped_ids: state.ungrouped,
      });
    return json({ items: [] });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/lists",
  useSearchParams: () => new URLSearchParams(),
}));
// Touch: "Move to group" flattens its items into the menu (no side
// submenu to hover open in jsdom).
vi.mock("@/lib/ui/use-coarse-pointer", async (orig) => ({
  ...(await orig<typeof CoarsePointer>()),
  useTouchDevice: () => true,
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: state.toastError,
    message: vi.fn(),
    info: state.toastInfo,
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}));
// Capture the Undo of a group delete instead of rendering a toast.
vi.mock("@/lib/ui/undo-toast", () => ({
  UNDO_TOAST_DURATION_MS: 8000,
  UndoFailedError: class UndoFailedError extends Error {},
  showUndoToast: (o: {
    onUndo: () => unknown;
    restoredMessage?: string | null | ((r: never) => string | null);
  }) => {
    state.undo.push(o.onUndo);
    state.undoOpts.push(o);
    return 1;
  },
}));

import { ReadingListsIndex } from "@/components/cbl/ReadingListsIndex";

/** `DELETE /me/view-groups/{id}` as the server does it: child groups
 *  take its place among its siblings, its lists follow its parent's (or
 *  the ungrouped order). */
function serverDelete(id: string) {
  const gone = state.groups.find((g) => g.id === id)!;
  const parent = gone.parent_id;
  const siblings = state.groups
    .filter((g) => g.parent_id === parent)
    .sort((a, b) => a.position - b.position);
  const kids = state.groups
    .filter((g) => g.parent_id === id)
    .sort((a, b) => a.position - b.position);
  const order = siblings.flatMap((g) => (g.id === id ? kids : [g]));
  state.groups = state.groups.flatMap((g) => {
    if (g.id === id) return [];
    const at = order.findIndex((o) => o.id === g.id);
    const moved = at < 0 ? g : { ...g, parent_id: parent, position: at };
    return moved.id === parent
      ? [{ ...moved, view_ids: [...moved.view_ids, ...gone.view_ids] }]
      : [moved];
  });
  if (parent === null) state.ungrouped = [...state.ungrouped, ...gone.view_ids];
}

function group(
  id: string,
  name: string,
  parent_id: string | null,
  position: number,
  view_ids: string[],
  collapsed = false,
): Group {
  return { id, scope: "cbl", name, parent_id, position, collapsed, view_ids };
}

const DAY = 24 * 60 * 60 * 1000;

function view(id: string, name: string, list: string) {
  return {
    id,
    user_id: "u1",
    kind: "cbl",
    name,
    description: null,
    custom_year_start: null,
    custom_year_end: null,
    custom_tags: [],
    match_mode: null,
    conditions: null,
    sort_field: null,
    sort_order: null,
    result_limit: null,
    cbl_list_id: list,
    // Equal everywhere: the view rows' own dates order nothing.
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    pinned: false,
    pinned_position: null,
    show_in_sidebar: false,
    is_system: false,
    system_key: null,
    icon: null,
    pinned_on_pages: [],
  };
}

function list(id: string, matched: number, read: number, daysAgo: number) {
  return {
    id,
    source_kind: "upload",
    parsed_name: id,
    parsed_matchers_present: true,
    imported_at: "2025-01-01T00:00:00Z",
    created_at: "2025-01-01T00:00:00Z",
    updated_at: "2025-01-01T00:00:00Z",
    last_refreshed_at: new Date(Date.now() - daysAgo * DAY).toISOString(),
    owner_user_id: "u1",
    stats: {
      total: matched,
      matched,
      read_count: read,
      ambiguous: 0,
      missing: 0,
      manual: 0,
    },
  };
}

beforeEach(() => {
  cleanup();
  window.localStorage.clear();
  state.calls = [];
  state.undo = [];
  state.refusePut = null;
  state.fail = [];
  state.undoOpts = [];
  state.toastError.mockClear();
  state.toastInfo.mockClear();
  // Stored by hand: Events = [Crisis, Watchmen]; the root = [Zeta, Alpha].
  // Most read: Watchmen (0.6) before Crisis (0.2); Alpha (1.0) before
  // Zeta (0.5). Recently updated: Watchmen (1 day) before Crisis (3).
  state.views = [
    view("v1", "Crisis", "l1"),
    view("v2", "Watchmen", "l2"),
    view("v3", "Alpha", "l3"),
    view("v4", "Zeta", "l4"),
  ];
  state.lists = [
    list("l1", 10, 2, 3),
    list("l2", 5, 3, 1),
    list("l3", 4, 4, 5),
    list("l4", 4, 2, 6),
  ];
  state.groups = [group("g1", "Events", null, 0, ["v1", "v2"])];
  state.ungrouped = ["v4", "v3"];
});

function renderIndex() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ReadingListsIndex />
    </QueryClientProvider>,
  );
}

const order = () =>
  [...document.querySelectorAll("[data-board-row] [data-row-name]")].map(
    (n) => n.textContent,
  );

async function sortByRead() {
  await screen.findByText("Crisis", { selector: "[data-row-name]" });
  const header = document.querySelector("[data-board-header]") as HTMLElement;
  fireEvent.click(within(header).getByRole("button", { name: "Read" }));
  await waitFor(() =>
    expect(order()).toEqual(["Watchmen", "Crisis", "Alpha", "Zeta"]),
  );
}

function openMenu(name: string) {
  fireEvent.pointerDown(screen.getByRole("button", { name }), {
    button: 0,
    ctrlKey: false,
  });
}

describe("commits while sorted", () => {
  it("Move to group sends the stored orders with the list appended to its target", async () => {
    renderIndex();
    await sortByRead();
    openMenu("Actions for Alpha");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Events" }));
    await waitFor(() =>
      expect(state.calls.some((c) => c.path === "/me/view-groups/layout")).toBe(
        true,
      ),
    );
    const put = state.calls.find((c) => c.path === "/me/view-groups/layout")!;
    expect(put.method).toBe("PUT");
    expect(put.body).toEqual({
      scope: "cbl",
      groups: [{ id: "g1", parent_id: null, view_ids: ["v1", "v2", "v3"] }],
      ungrouped: ["v4"],
    });
    // The sort stays on, and the moved list shows in its sorted place.
    await waitFor(() =>
      expect(order()).toEqual(["Alpha", "Watchmen", "Crisis", "Zeta"]),
    );
  });

  it("Undo of a group delete restores the stored order of the root and the group", async () => {
    renderIndex();
    await sortByRead();
    openMenu("Actions for group Events");
    fireEvent.click(
      await screen.findByRole("menuitem", { name: /Delete group/ }),
    );
    await waitFor(() => expect(state.undo).toHaveLength(1));
    // The delete itself sends no order: just the DELETE.
    expect(state.calls.map((c) => `${c.method} ${c.path}`)).toEqual([
      "DELETE /me/view-groups/g1",
    ]);

    // The server moved the members to the end of the ungrouped order.
    expect(state.ungrouped).toEqual(["v4", "v3", "v1", "v2"]);
    await state.undo[0]!();
    const create = state.calls.find(
      (c) => c.method === "POST" && c.path === "/me/view-groups",
    )!;
    expect(create.body).toEqual({
      scope: "cbl",
      name: "Events",
      view_ids: [],
    });
    const put = state.calls.find((c) => c.path === "/me/view-groups/layout")!;
    expect(put.body).toEqual({
      scope: "cbl",
      groups: [{ id: "g-new", parent_id: null, view_ids: ["v1", "v2"] }],
      ungrouped: ["v4", "v3"],
    });
  });
});

describe("sort menu", () => {
  it('"Recently updated" orders the rows by the list\'s own date', async () => {
    renderIndex();
    await screen.findByText("Crisis", { selector: "[data-row-name]" });
    expect(order()).toEqual(["Crisis", "Watchmen", "Zeta", "Alpha"]);
    const trigger = screen.getByRole("combobox", { name: "Sort" });
    fireEvent.pointerDown(trigger, {
      button: 0,
      ctrlKey: false,
      pointerType: "mouse",
    });
    const options = await screen.findAllByRole("option");
    // The view-row "updated" sort is replaced, not doubled.
    expect(
      options.filter((o) => o.textContent === "Recently updated"),
    ).toHaveLength(1);
    fireEvent.click(screen.getByRole("option", { name: "Recently updated" }));
    // Watchmen (1 day) before Crisis (3); Alpha (5) before Zeta (6). By
    // the view rows' equal dates nothing would move.
    await waitFor(() =>
      expect(order()).toEqual(["Watchmen", "Crisis", "Alpha", "Zeta"]),
    );
  });
});

describe("nested groups", () => {
  // Marvel ⊃ Events ⊃ Big ⊃ Deep (four levels), DC at the top.
  // Marvel = [Crisis, Zeta] by hand (Most read: Zeta 0.5 before Crisis
  // 0.2),
  // Events = [Watchmen]; ungrouped = [Alpha].
  beforeEach(() => {
    state.groups = [
      group("m", "Marvel", null, 0, ["v1", "v4"]),
      group("e", "Events", "m", 0, ["v2"]),
      group("b", "Big", "e", 0, []),
      group("x", "Deep", "b", 0, []),
      group("d", "DC", null, 1, []),
    ];
    state.ungrouped = ["v3"];
  });

  const tree = [
    { id: "m", parent_id: null, view_ids: ["v1", "v4"] },
    { id: "e", parent_id: "m", view_ids: ["v2"] },
    { id: "b", parent_id: "e", view_ids: [] },
    { id: "x", parent_id: "b", view_ids: [] },
    { id: "d", parent_id: null, view_ids: [] },
  ];

  async function ready(sorted: boolean) {
    renderIndex();
    await screen.findByText("Crisis", { selector: "[data-row-name]" });
    if (sorted) {
      const header = document.querySelector(
        "[data-board-header]",
      ) as HTMLElement;
      fireEvent.click(within(header).getByRole("button", { name: "Read" }));
      // Inside Marvel: Zeta (0.5) before Crisis (0.2), not the stored
      // [Crisis, Zeta].
      await waitFor(() =>
        expect(order()).toEqual(["Watchmen", "Zeta", "Crisis", "Alpha"]),
      );
    }
  }

  const lastPut = () =>
    state.calls.filter((c) => c.path === "/me/view-groups/layout").at(-1);

  it("renders the tree: folders nested with their levels, rows indented", async () => {
    await ready(false);
    const big = screen.getByRole("region", { name: "Group Big, level 3" });
    expect(
      within(big).getByRole("region", { name: "Group Deep, level 4" }),
    ).toBeTruthy();
    const marvel = screen.getByRole("region", { name: "Group Marvel" });
    expect(
      within(marvel).getAllByRole("button", { expanded: true }).length,
    ).toBeGreaterThan(0);
    // Totals cover the lists of the folders below.
    expect(within(marvel).getByText("3 lists")).toBeTruthy();
    // Collapsing a folder hides everything under it.
    expect(
      (
        document.getElementById(
          within(marvel)
            .getByRole("button", { name: "Collapse Marvel" })
            .getAttribute("aria-controls")!,
        ) as HTMLElement
      ).hidden,
    ).toBe(false);
  });

  for (const sorted of [false, true]) {
    const when = sorted ? " while sorted" : "";

    it(`moves a list into a nested folder by menu${when}`, async () => {
      await ready(sorted);
      openMenu("Actions for Alpha");
      fireEvent.click(await screen.findByRole("menuitem", { name: "Big" }));
      await waitFor(() => expect(lastPut()).toBeDefined());
      expect(lastPut()!.body).toEqual({
        scope: "cbl",
        groups: tree.map((g) =>
          g.id === "b" ? { ...g, view_ids: ["v3"] } : g,
        ),
        ungrouped: [],
      });
    });

    it(`moves a folder into another by menu${when}`, async () => {
      await ready(sorted);
      openMenu("Actions for group DC");
      fireEvent.click(await screen.findByRole("menuitem", { name: "Big" }));
      await waitFor(() => expect(lastPut()).toBeDefined());
      expect(lastPut()!.body).toEqual({
        scope: "cbl",
        groups: [
          tree[0],
          tree[1],
          tree[2],
          tree[3],
          { id: "d", parent_id: "b", view_ids: [] },
        ],
        ungrouped: ["v3"],
      });
    });

    it(`deletes a nested folder and Undo restores the exact tree${when}`, async () => {
      await ready(sorted);
      openMenu("Actions for group Events");
      fireEvent.click(
        await screen.findByRole("menuitem", { name: /Delete group/ }),
      );
      await waitFor(() => expect(state.undo).toHaveLength(1));
      expect(state.calls.map((c) => `${c.method} ${c.path}`)).toEqual([
        "DELETE /me/view-groups/e",
      ]);
      // On the server: Big moved up into Marvel, Watchmen joined Marvel.
      expect(state.groups.find((g) => g.id === "b")!.parent_id).toBe("m");
      expect(state.groups.find((g) => g.id === "m")!.view_ids).toEqual([
        "v1",
        "v4",
        "v2",
      ]);
      // The page shows the lift right away.
      await waitFor(() =>
        expect(
          screen.queryByRole("region", { name: /Group Events/ }),
        ).toBeNull(),
      );
      expect(
        screen.getByRole("region", { name: "Group Big, level 2" }),
      ).toBeTruthy();

      await state.undo[0]!();
      const create = state.calls.find((c) => c.method === "POST")!;
      expect(create.body).toEqual({
        scope: "cbl",
        name: "Events",
        view_ids: [],
      });
      expect(lastPut()!.body).toEqual({
        scope: "cbl",
        groups: [
          tree[0],
          { id: "g-new", parent_id: "m", view_ids: ["v2"] },
          { id: "b", parent_id: "g-new", view_ids: [] },
          tree[3],
          tree[4],
        ],
        ungrouped: ["v3"],
      });
    });
  }

  it("collapsing hides a whole branch; a search opens it again", async () => {
    state.groups = state.groups.map((g) =>
      g.id === "m" ? { ...g, collapsed: true } : g,
    );
    state.groups = state.groups.map((g) =>
      g.id === "x" ? { ...g, view_ids: ["v3"] } : g,
    );
    state.ungrouped = [];
    renderIndex();
    await screen.findByRole("region", { name: "Group Marvel" });
    const alpha = () =>
      [...document.querySelectorAll("[data-board-row]")].find((r) =>
        r.textContent?.includes("Alpha"),
      )!;
    const shown = (el: Element) => !el.closest("[hidden]");
    expect(shown(alpha())).toBe(false);
    expect(
      screen
        .getByRole("button", { name: "Expand Marvel" })
        .getAttribute("aria-expanded"),
    ).toBe("false");
    fireEvent.change(screen.getByRole("searchbox", { name: "Search lists" }), {
      target: { value: "alph" },
    });
    await waitFor(() => expect(shown(alpha())).toBe(true));
  });

  it("carries list ids this page doesn't know in place when it saves", async () => {
    // Filed into Marvel from another tab; this page has no such view.
    state.groups = state.groups.map((g) =>
      g.id === "m" ? { ...g, view_ids: ["v1", "v-other-tab", "v4"] } : g,
    );
    await ready(false);
    openMenu("Actions for Alpha");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Big" }));
    await waitFor(() => expect(lastPut()).toBeDefined());
    const sent = lastPut()!.body as { groups: typeof tree };
    expect(sent.groups[0]).toEqual({
      id: "m",
      parent_id: null,
      view_ids: ["v1", "v-other-tab", "v4"],
    });
  });

  it("Undo removes the group it re-created when the restore is refused", async () => {
    await ready(false);
    openMenu("Actions for group Events");
    fireEvent.click(
      await screen.findByRole("menuitem", { name: /Delete group/ }),
    );
    await waitFor(() => expect(state.undo).toHaveLength(1));
    state.calls = [];
    // Another tab changed the tree between the fetch and the PUT.
    state.refusePut = "groups must list every group in this scope exactly once";
    await expect(state.undo[0]!()).rejects.toThrow(
      "Couldn't restore the group. Nothing was changed.",
    );
    expect(state.calls.map((c) => `${c.method} ${c.path}`)).toEqual([
      "POST /me/view-groups",
      "PUT /me/view-groups/layout",
      "DELETE /me/view-groups/g-new",
    ]);
    expect(state.groups.some((g) => g.id === "g-new")).toBe(false);
  });

  const undoCalls = () => state.calls.map((c) => `${c.method} ${c.path}`);
  async function deleteEvents() {
    openMenu("Actions for group Events");
    fireEvent.click(
      await screen.findByRole("menuitem", { name: /Delete group/ }),
    );
    await waitFor(() => expect(state.undo).toHaveLength(1));
    state.calls = [];
  }

  it("Undo keeps the group when the restore's answer is lost, and says so", async () => {
    await ready(false);
    await deleteEvents();
    // The PUT committed; its response never arrived.
    state.fail = [{ call: "PUT /me/view-groups/layout", status: "lost" }];
    await expect(state.undo[0]!()).rejects.toThrow(
      "The restore may not have finished. The page was refreshed.",
    );
    expect(undoCalls()).toEqual([
      "POST /me/view-groups",
      "PUT /me/view-groups/layout",
    ]);
    // The restore that landed stands: Events is back, with Big under it.
    expect(state.groups.find((g) => g.id === "g-new")?.parent_id).toBe("m");
    expect(state.groups.find((g) => g.id === "b")?.parent_id).toBe("g-new");
  });

  it("Undo treats a 5xx on the restore like a lost answer: nothing deleted", async () => {
    await ready(false);
    await deleteEvents();
    state.fail = [{ call: "PUT /me/view-groups/layout", status: 503 }];
    await expect(state.undo[0]!()).rejects.toThrow(
      "The restore may not have finished. The page was refreshed.",
    );
    expect(undoCalls()).toEqual([
      "POST /me/view-groups",
      "PUT /me/view-groups/layout",
    ]);
  });

  it("Undo removes the re-created group when restoring its collapsed state fails", async () => {
    state.groups = state.groups.map((g) =>
      g.id === "e" ? { ...g, collapsed: true } : g,
    );
    await ready(false);
    await deleteEvents();
    state.fail = [{ call: "PATCH /me/view-groups/g-new", status: 500 }];
    await expect(state.undo[0]!()).rejects.toThrow(
      "Couldn't restore the group. Nothing was changed.",
    );
    // Nothing was restored yet, so the empty group goes, whatever the
    // PATCH failure was.
    expect(undoCalls()).toEqual([
      "POST /me/view-groups",
      "PATCH /me/view-groups/g-new",
      "DELETE /me/view-groups/g-new",
    ]);
    expect(state.groups.some((g) => g.id === "g-new")).toBe(false);
  });

  it("says so when the cleanup after a refused restore fails too", async () => {
    await ready(false);
    await deleteEvents();
    state.refusePut = "groups must list every group in this scope exactly once";
    state.fail = [{ call: "DELETE /me/view-groups/g-new", status: 500 }];
    await expect(state.undo[0]!()).rejects.toThrow(
      'Couldn\'t restore the group. An empty "Events" was left at the top level.',
    );
    expect(undoCalls()).toEqual([
      "POST /me/view-groups",
      "PUT /me/view-groups/layout",
      "DELETE /me/view-groups/g-new",
    ]);
  });

  it("the restored toast names the groups the depth cap moved up", async () => {
    await ready(false);
    await deleteEvents();
    const done = state.undoOpts[0]!.restoredMessage as (n: number) => string;
    expect(done(0)).toBe('Restored group "Events"');
    expect(done(2)).toBe(
      'Restored group "Events". 2 groups moved up to stay within four levels.',
    );
    // An ordinary restore moved nothing, and resolves to that count.
    await expect(state.undo[0]!()).resolves.toBe(0);
  });

  it("a move on a page showing a parent it couldn't place is refused, refreshed and said", async () => {
    // DC's parent is gone from this page's copy: it shows at the top
    // level only as a fallback.
    state.groups = state.groups.map((g) =>
      g.id === "d" ? { ...g, parent_id: "gone" } : g,
    );
    await ready(false);
    openMenu("Actions for Alpha");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Big" }));
    await waitFor(() =>
      expect(state.toastInfo).toHaveBeenCalledWith(
        "This page was out of date and has been refreshed. Try the move again.",
      ),
    );
    expect(lastPut()).toBeUndefined();
  });

  it("a refused nesting move rolls the page back and shows the error", async () => {
    await ready(false);
    state.refusePut = "groups nest at most 4 levels deep";
    openMenu("Actions for group DC");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Big" }));
    await waitFor(() =>
      expect(state.toastError).toHaveBeenCalledWith(
        "groups nest at most 4 levels deep",
        expect.anything(),
      ),
    );
    // DC is back at the top level, not under Big.
    await waitFor(() =>
      expect(
        screen.getByRole("region", { name: "Group DC" }).closest("li"),
      ).toBeNull(),
    );
    expect(state.groups.find((g) => g.id === "d")!.parent_id).toBeNull();
  });

  it("names colliding groups by their path in the move menus", async () => {
    state.groups = [
      ...state.groups,
      group("m2", "Annuals", "m", 1, []),
      group("d2", "Annuals", "d", 0, []),
    ];
    await ready(false);
    openMenu("Actions for Alpha");
    expect(
      await screen.findByRole("menuitem", { name: "Marvel / Annuals" }),
    ).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "DC / Annuals" })).toBeTruthy();
    // A unique name stays short.
    expect(screen.getByRole("menuitem", { name: "Big" })).toBeTruthy();
  });

  it("disables what the depth cap rules out, with the reason", async () => {
    await ready(false);
    // Deep is on the fourth level: nothing can go inside it.
    openMenu("Actions for group Deep");
    const inside = await screen.findByRole("menuitem", {
      name: /New group inside/,
    });
    expect(inside.getAttribute("aria-disabled")).toBe("true");
    expect(inside.textContent).toContain("at most 4 levels");
    fireEvent.keyDown(inside, { key: "Escape" });
    cleanup();

    // DC can go into Big (level 4) but not into Deep (level 5).
    await ready(false);
    openMenu("Actions for group DC");
    const deep = await screen.findByRole("menuitem", { name: /^Deep/ });
    expect(deep.getAttribute("aria-disabled")).toBe("true");
    expect(deep.textContent).toContain("Too deep");
    expect(
      screen
        .getByRole("menuitem", { name: "Big" })
        .getAttribute("aria-disabled"),
    ).toBeNull();
    // The current place is disabled; Marvel's own subtree isn't offered
    // to Marvel at all.
    expect(
      screen
        .getByRole("menuitem", { name: "Top level" })
        .getAttribute("aria-disabled"),
    ).toBe("true");
    cleanup();

    // Marvel spans four levels: only the top level fits it, and it's
    // already there, so DC is refused for depth.
    await ready(false);
    openMenu("Actions for group Marvel");
    const items = (await screen.findAllByRole("menuitem")).map(
      (m) => m.textContent,
    );
    expect(items.some((t) => t?.startsWith("Events"))).toBe(false);
    expect(
      screen
        .getByRole("menuitem", { name: /^DC/ })
        .getAttribute("aria-disabled"),
    ).toBe("true");
    expect(
      screen
        .getByRole("menuitem", { name: /New group inside/ })
        .getAttribute("aria-disabled"),
    ).toBeNull();
  });
});
