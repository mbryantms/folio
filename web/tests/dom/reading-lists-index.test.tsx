// @vitest-environment jsdom
/**
 * The reader-side `/lists` index: the sidebar "Reading lists" entry lands
 * on the same board Settings → Views shows on its Reading lists tab. The
 * real query hooks run against a mocked `apiFetch`: own and shared CBL
 * views both render (filter views and collections do not), each linking
 * to its detail view; the empty state and the Import CBL entry follow
 * `canImport`.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const state = vi.hoisted(() => ({ views: [] as unknown[] }));

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
    if (path.startsWith("/me/cbl-lists")) return json({ items: [] });
    if (path.startsWith("/me/view-groups"))
      return json({ scope: "cbl", groups: [], ungrouped_ids: [] });
    return json({ items: [] });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/lists",
  useSearchParams: () => new URLSearchParams(),
}));

import { ReadingListsIndex } from "@/components/cbl/ReadingListsIndex";

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

function renderIndex(canImport: boolean) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ReadingListsIndex canImport={canImport} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  state.views = [];
});

describe("<ReadingListsIndex>", () => {
  it("renders own and shared reading lists, linking to their detail views", async () => {
    state.views = [
      view({ id: "own-1", name: "My Crisis order", cbl_list_id: "l1" }),
      view({
        id: "shared-1",
        name: "Starter: Batman",
        user_id: null,
        is_system: true,
        cbl_list_id: "l2",
      }),
      view({ id: "f1", name: "Unread capes", kind: "filter_series" }),
      view({ id: "c1", name: "Favourites", kind: "collection" }),
    ];
    renderIndex(true);

    const own = await screen.findByText("My Crisis order");
    expect(own.closest("a")?.getAttribute("href")).toBe("/views/own-1");
    const shared = screen.getByText("Starter: Batman");
    expect(shared.closest("a")?.getAttribute("href")).toBe("/views/shared-1");
    expect(screen.queryByText("Unread capes")).toBeNull();
    expect(screen.queryByText("Favourites")).toBeNull();
    expect(screen.getByRole("button", { name: /Import CBL/ })).toBeTruthy();
  });

  it("shows the empty state with the import entry for a user who can import", async () => {
    renderIndex(true);
    expect(
      await screen.findByText(
        "No reading lists yet — import a CBL to track a reading order.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: /Import CBL/ })).toBeTruthy();
  });

  it("drops the import entry when the caller may not import", async () => {
    renderIndex(false);
    expect(await screen.findByText("No reading lists yet.")).toBeTruthy();
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: /Import CBL/ })).toBeNull(),
    );
  });
});
