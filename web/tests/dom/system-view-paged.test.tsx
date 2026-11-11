// @vitest-environment jsdom
/**
 * The Continue reading, On deck and Recently read "see all" pages walk
 * their endpoint's cursor (`?limit=&cursor=`) with `useInfiniteQuery` and
 * the shared `<InfiniteSentinel>`: the first page renders, the sentinel
 * fetches the next one, and the caption is the server's `total` from the
 * first page. On deck says so when its bounded merge hit the bound.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.stubGlobal(
  "IntersectionObserver",
  class {
    private live = true;
    constructor(private cb: IntersectionObserverCallback) {}
    observe(el: Element) {
      queueMicrotask(() => {
        if (!this.live) return;
        this.cb(
          [{ isIntersecting: true, target: el } as IntersectionObserverEntry],
          this as unknown as IntersectionObserver,
        );
      });
    }
    unobserve() {}
    disconnect() {
      this.live = false;
    }
  },
);

const net = vi.hoisted(() => ({
  calls: [] as string[],
  boundedTo: undefined as number | undefined,
}));

function card(id: string) {
  return {
    kind: "series_next",
    issue: { id, series_id: `s-${id}` },
    series_name: "S",
    progress: { last_page: 1, percent: 0.1, updated_at: "" },
    last_activity: "",
  };
}

vi.mock("@/lib/api/auth-refresh", () => ({
  getCsrfToken: () => null,
  apiFetch: async (path: string) => {
    net.calls.push(path);
    const second = path.includes("cursor=");
    const body = second
      ? { items: [card("c")], next_cursor: null }
      : {
          items: [card("a"), card("b")],
          next_cursor: "page-2",
          total: 3,
          ...(net.boundedTo != null ? { bounded_to: net.boundedTo } : {}),
        };
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  },
}));
vi.mock("@/components/library/ProgressIssueCard", () => ({
  ProgressIssueCard: ({ card }: { card: { issue: { id: string } } }) => (
    <div data-testid="card">{card.issue.id}</div>
  ),
  ProgressIssueCardSkeleton: () => null,
}));
vi.mock("@/components/library/OnDeckCard", () => ({
  OnDeckCard: ({ card }: { card: { issue: { id: string } } }) => (
    <div data-testid="card">{card.issue.id}</div>
  ),
  OnDeckCardSkeleton: () => null,
}));
vi.mock("@/components/library/PhoneControlsBar", () => ({
  PhoneControlsBar: () => null,
}));

import { SystemViewDetail } from "@/components/saved-views/SystemViewDetail";
import type { SavedViewView } from "@/lib/api/types";

function view(system_key: string, name: string): SavedViewView {
  return {
    id: `system:${system_key}`,
    kind: "system",
    system_key,
    name,
    description: null,
  } as unknown as SavedViewView;
}

function renderPage(key: string, name: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SystemViewDetail view={view(key, name)} />
    </QueryClientProvider>,
  );
}

const PAGES = [
  ["continue_reading", "Continue reading", "/me/continue-reading"],
  ["on_deck", "On deck", "/me/on-deck"],
  ["recent_history", "Recently read", "/me/recent-history"],
] as const;

beforeEach(() => {
  net.calls = [];
  net.boundedTo = undefined;
});

describe("paged progress-rail pages", () => {
  for (const [key, name, path] of PAGES) {
    it(`${name}: renders page one, fetches page two, shows the total`, async () => {
      const { container } = renderPage(key, name);
      // The first page asks for a page size (and with it, the total).
      await waitFor(() => expect(net.calls[0]).toBe(`${path}?limit=48`));
      expect(await screen.findByText("3 items")).toBeTruthy();
      // The sentinel fires and the next page is requested with its cursor.
      await waitFor(() =>
        expect(screen.getAllByTestId("card").map((c) => c.textContent)).toEqual(
          ["a", "b", "c"],
        ),
      );
      expect(net.calls).toEqual([
        `${path}?limit=48`,
        `${path}?limit=48&cursor=page-2`,
      ]);
      expect(container.textContent).not.toMatch(/Most recent/);
    });
  }

  it("On deck: a bounded merge says so in the caption", async () => {
    net.boundedTo = 200;
    renderPage("on_deck", "On deck");
    expect(
      await screen.findByText(
        "3 items, from your 200 most recently read series and reading lists",
      ),
    ).toBeTruthy();
  });
});
