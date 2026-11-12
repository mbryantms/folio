// @vitest-environment jsdom
/**
 * What the board sends while a sort is on, through the real component:
 * the layout PUT after "Move to group" and the layout Undo restores
 * after a group delete carry the stored, hand-arranged order (a moved
 * list appended to its target), never the sorted order on screen. Also:
 * "Recently updated" in the sort menu orders rows by the list's own
 * date. The query hooks run against a stateful mocked `apiFetch`.
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
    if (path.startsWith("/auth/me")) return json({ id: "u1", role: "user" });
    if (path.startsWith("/me/saved-views")) return json({ items: state.views });
    if (path.startsWith("/me/cbl-lists")) return json({ items: state.lists });
    if (path === "/me/view-groups" && method === "POST") {
      const g: Group = {
        id: "g-new",
        scope: "cbl",
        name: body.name,
        position: state.groups.length,
        collapsed: false,
        view_ids: body.view_ids,
      };
      state.groups = [...state.groups, g];
      return json(g, 201);
    }
    if (path.startsWith("/me/view-groups/") && method === "DELETE") {
      const id = path.split("/").pop();
      state.groups = state.groups.filter((g) => g.id !== id);
      return new Response(null, { status: 204 });
    }
    if (path === "/me/view-groups/layout" && method === "PUT") {
      const byId = new Map(state.groups.map((g) => [g.id, g]));
      state.groups = body.groups.map(
        (g: { id: string; view_ids: string[] }, i: number) => ({
          ...byId.get(g.id)!,
          position: i,
          view_ids: g.view_ids,
        }),
      );
      state.ungrouped = body.ungrouped;
      return new Response(null, { status: 204 });
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
// Capture the Undo of a group delete instead of rendering a toast.
vi.mock("@/lib/ui/undo-toast", () => ({
  UNDO_TOAST_DURATION_MS: 8000,
  showUndoToast: (o: { onUndo: () => unknown }) => {
    state.undo.push(o.onUndo);
    return 1;
  },
}));

import { ReadingListsIndex } from "@/components/cbl/ReadingListsIndex";

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
      groups: [{ id: "g1", view_ids: ["v1", "v2", "v3"] }],
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

    // The server ungroups the members (no pinned position).
    await state.undo[0]!();
    const create = state.calls.find(
      (c) => c.method === "POST" && c.path === "/me/view-groups",
    )!;
    expect(create.body).toEqual({
      scope: "cbl",
      name: "Events",
      view_ids: ["v1", "v2"],
    });
    const put = state.calls.find((c) => c.path === "/me/view-groups/layout")!;
    expect(put.body).toEqual({
      scope: "cbl",
      groups: [{ id: "g-new", view_ids: ["v1", "v2"] }],
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
