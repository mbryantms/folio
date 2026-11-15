// @vitest-environment jsdom
/**
 * `ViewCard` is one link (WP-9.1): the name is the only `<a>`, its
 * `::after` stretches over the card so the cover stack opens the view
 * too, and the controls that keep their own click or tooltip (the ⋯
 * menu, the reading-list stats) are lifted above the stretch. jsdom has
 * no layout, so this pins the structure the stretch depends on; the
 * click and the drag are covered in `tests/e2e/view-card-link.spec.ts`.
 */
import { cleanup, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Folder, List, Sparkles } from "lucide-react";
import { afterEach, describe, expect, it, vi } from "vitest";

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
    if (path.startsWith("/me/view-covers"))
      return json({
        items: [{ view_id: "c1", cover_urls: ["/issues/i1/pages/0/thumb"] }],
      });
    return json({ items: [] });
  },
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/collections",
  useSearchParams: () => new URLSearchParams(),
}));

import { CblListsContext } from "@/components/cbl/cbl-lists-context";
import { ViewCard } from "@/components/saved-views/ViewsIndex";
import type { CblListView, SavedViewView } from "@/lib/api/types";

function view(over: Partial<SavedViewView>): SavedViewView {
  return {
    id: "c1",
    user_id: "u1",
    kind: "collection",
    name: "Favourites",
    description: "The ones worth rereading",
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
    show_in_sidebar: true,
    is_system: false,
    system_key: null,
    icon: null,
    pinned_on_pages: ["p1"],
    ...over,
  } as SavedViewView;
}

function renderCard(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>{ui}</QueryClientProvider>,
  );
}

/** True when `el` or an ancestor inside the card carries a z-index
 *  utility higher than the stretched link's `after:z-[1]`. */
function liftedAboveLink(el: Element, card: Element): boolean {
  for (let n: Element | null = el; n && n !== card; n = n.parentElement) {
    if (/(^|\s)z-\[(\d+)\]/.test(n.className)) {
      const z = Number(/(^|\s)z-\[(\d+)\]/.exec(n.className)![2]);
      if (z > 1 && /(^|\s)(relative|absolute)(\s|$)/.test(n.className)) {
        return true;
      }
    }
  }
  return false;
}

afterEach(cleanup);

describe("ViewCard is one link", () => {
  it("a collection card: one link named after the view, stretched over the card", async () => {
    const { container } = renderCard(
      <ViewCard view={view({})} Icon={Folder} isCollection size="large" />,
    );
    const card = container.querySelector("[data-view-card]")!;
    expect(card).toBeTruthy();
    // The stretch is positioned against the card.
    expect(card.className).toMatch(/(^|\s)relative(\s|$)/);

    const links = within(card as HTMLElement).getAllByRole("link");
    expect(links).toHaveLength(1);
    const link = links[0]!;
    // One accessible name: the view's name, not name + description.
    expect(link.textContent).toBe("Favourites");
    expect(link.getAttribute("href")).toBe("/views/c1");
    expect(link.className).toContain("after:absolute");
    expect(link.className).toContain("after:inset-0");
    // The ring is on the stretch, so focus outlines the whole card.
    expect(link.className).toContain("focus-visible:after:ring-2");
    // The description is still on the card, outside the link's name.
    expect(
      screen.getByText("The ones worth rereading").closest("a"),
    ).toBeNull();

    // The cover stack is decoration under the stretch, never a second link.
    const stack = card.querySelector(".cover-stack")!;
    expect(stack).toBeTruthy();
    expect(stack.getAttribute("aria-hidden")).toBe("true");
    expect(stack.closest("a")).toBeNull();
    expect(liftedAboveLink(stack, card)).toBe(false);

    // The ⋯ menu keeps its own click.
    const menu = within(card as HTMLElement).getByRole("button");
    expect(liftedAboveLink(menu, card)).toBe(true);

    // Plain badges have nothing of their own to click or hover, so they
    // stay under the stretch and open the view like the rest of the card.
    const badge = screen.getByText("Sidebar");
    expect(liftedAboveLink(badge, card)).toBe(false);
  });

  it("a filter-view card (no cover stack) lifts its menu too", () => {
    const { container } = renderCard(
      <ViewCard
        view={view({ id: "f1", kind: "filter_series", name: "Unread Image" })}
        Icon={Sparkles}
        isCollection={false}
      />,
    );
    const card = container.querySelector("[data-view-card]")!;
    expect(within(card as HTMLElement).getAllByRole("link")).toHaveLength(1);
    expect(card.querySelector(".cover-stack")).toBeNull();
    const menu = within(card as HTMLElement).getByRole("button");
    expect(liftedAboveLink(menu, card)).toBe(true);
  });

  it("a reading-list card keeps its stats' tooltips above the link", () => {
    const list = {
      id: "l1",
      imported_at: "2025-01-01T00:00:00Z",
      last_refreshed_at: null,
      stats: {
        total: 13,
        matched: 10,
        read_count: 4,
        ambiguous: 0,
        missing: 3,
        manual: 0,
      },
    } as unknown as CblListView;
    const { container } = renderCard(
      <CblListsContext.Provider value={new Map([["l1", list]])}>
        <ViewCard
          view={view({
            id: "v1",
            kind: "cbl",
            name: "Crisis order",
            cbl_list_id: "l1",
          })}
          Icon={List}
          isCollection={false}
        />
      </CblListsContext.Provider>,
    );
    const card = container.querySelector("[data-view-card]")!;
    expect(within(card as HTMLElement).getAllByRole("link")).toHaveLength(1);
    const pill = screen.getByTitle(
      "10 of 13 entries collected in your library",
    );
    expect(liftedAboveLink(pill, card)).toBe(true);
  });
});
