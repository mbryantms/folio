// @vitest-environment jsdom
/**
 * Settings → Pages "Hidden from rails" (audit 2026-11 N-O4, §5 #9). The
 * section lists `GET /me/rail-dismissals` with a real infinite query and
 * QueryClient; only the network (`apiFetch`), the IntersectionObserver and
 * the restore mutation hook are faked.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { RailDismissalListView, RailDismissalView } from "@/lib/api/types";

const net = vi.hoisted(() => ({
  urls: [] as string[],
  pages: new Map<string, RailDismissalListView>(),
}));

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (path: string) => {
    net.urls.push(path);
    const url = new URL(path, "http://test");
    if (url.searchParams.get("cursor") === "broken") {
      return new Response(
        JSON.stringify({ error: { code: "internal", message: "internal" } }),
        { status: 500, headers: { "Content-Type": "application/json" } },
      );
    }
    const body = net.pages.get(url.searchParams.get("cursor") ?? "") ?? {
      items: [],
      next_cursor: null,
      total: 0,
      unavailable: 0,
    };
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }),
  getCsrfToken: () => "csrf",
}));

const restore = vi.hoisted(() => ({ mutate: vi.fn() }));
vi.mock("@/lib/api/mutations", () => ({
  useRestoreRailItem: () => ({ mutate: restore.mutate, isPending: false }),
}));

const io = vi.hoisted(() => ({
  live: new Set<{ cb: IntersectionObserverCallback; el: Element | null }>(),
}));
vi.stubGlobal(
  "IntersectionObserver",
  class {
    private entry: { cb: IntersectionObserverCallback; el: Element | null };
    constructor(cb: IntersectionObserverCallback) {
      this.entry = { cb, el: null };
    }
    observe(el: Element) {
      this.entry.el = el;
      io.live.add(this.entry);
    }
    unobserve() {}
    disconnect() {
      io.live.delete(this.entry);
    }
  },
);
function intersect() {
  for (const o of [...io.live]) {
    o.cb(
      [{ isIntersecting: true, target: o.el } as IntersectionObserverEntry],
      {} as IntersectionObserver,
    );
  }
}

import {
  HiddenRailItems,
  hiddenFromLabel,
} from "@/components/settings/HiddenRailItems";

const issueRow: RailDismissalView = {
  id: "issue:abc",
  target_kind: "issue",
  target_id: "abc",
  hidden_from: ["continue_reading", "recent_history"],
  dismissed_at: "2026-10-30T12:00:00Z",
  target: {
    name: "Saga #1",
    subtitle: "Chapter One",
    slug: "saga-1",
    series_slug: "saga",
    saved_view_id: null,
    cover_url: "/issues/abc/pages/0/thumb",
  },
};

const seriesRow: RailDismissalView = {
  id: "series:s-1",
  target_kind: "series",
  target_id: "s-1",
  hidden_from: ["on_deck", "random_picks", "similar_series"],
  dismissed_at: "2026-10-29T12:00:00Z",
  target: {
    name: "Paper Girls",
    subtitle: "2015",
    slug: "paper-girls",
    series_slug: null,
    saved_view_id: null,
    cover_url: null,
  },
};

const cblRow: RailDismissalView = {
  id: "cbl:l-1",
  target_kind: "cbl",
  target_id: "l-1",
  hidden_from: ["on_deck"],
  dismissed_at: "2026-10-28T12:00:00Z",
  target: {
    name: "Event order",
    subtitle: null,
    slug: null,
    series_slug: null,
    saved_view_id: "v-9",
    cover_url: null,
  },
};

function renderSection() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={qc}>
      <HiddenRailItems />
    </QueryClientProvider>,
  );
}

describe("<HiddenRailItems>", () => {
  beforeEach(() => {
    net.urls.length = 0;
    net.pages.clear();
    io.live.clear();
    restore.mutate.mockReset();
  });

  it("renders a row per hidden item with its link, rails and time", async () => {
    net.pages.set("", {
      items: [issueRow, seriesRow, cblRow],
      next_cursor: null,
      total: 3,
      unavailable: 0,
    });
    renderSection();

    const issueLink = await screen.findByRole("link", { name: "Saga #1" });
    expect(issueLink.getAttribute("href")).toBe("/series/saga/issues/saga-1");
    expect(
      screen.getByRole("link", { name: "Paper Girls" }).getAttribute("href"),
    ).toBe("/series/paper-girls");
    expect(
      screen.getByRole("link", { name: "Event order" }).getAttribute("href"),
    ).toBe("/views/v-9");
    expect(screen.getByText("Chapter One")).toBeTruthy();
    expect(
      screen.getByText(/Hidden from Continue reading, Recently read/),
    ).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /^Restore / })).toHaveLength(
      3,
    );
    expect(net.urls).toEqual(["/me/rail-dismissals"]);
  });

  it("Restore calls the restore mutation with the row's kind and id", async () => {
    net.pages.set("", {
      items: [seriesRow],
      next_cursor: null,
      total: 1,
      unavailable: 0,
    });
    renderSection();
    fireEvent.click(
      await screen.findByRole("button", { name: "Restore Paper Girls" }),
    );
    expect(restore.mutate).toHaveBeenCalledWith({
      target_kind: "series",
      target_id: "s-1",
    });
  });

  it("shows the empty state and the unavailable count", async () => {
    net.pages.set("", {
      items: [],
      next_cursor: null,
      total: 0,
      unavailable: 2,
    });
    renderSection();
    expect(await screen.findByText("Nothing hidden")).toBeTruthy();
    expect(
      screen.getByText(/2 hidden items are no longer available/),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Restore/ })).toBeNull();
  });

  it("fetches page two when the sentinel intersects, then hides it", async () => {
    net.pages.set("", {
      items: [issueRow],
      next_cursor: "c-2",
      total: 2,
      unavailable: 0,
    });
    net.pages.set("c-2", {
      items: [seriesRow],
      next_cursor: null,
    });
    renderSection();
    await screen.findByRole("link", { name: "Saga #1" });
    expect(screen.getByTestId("infinite-sentinel").className).not.toContain(
      "hidden",
    );

    await act(async () => {
      intersect();
    });
    await screen.findByRole("link", { name: "Paper Girls" });
    expect(net.urls).toEqual([
      "/me/rail-dismissals",
      "/me/rail-dismissals?cursor=c-2",
    ]);
    await waitFor(() =>
      expect(screen.getByTestId("infinite-sentinel").className).toContain(
        "hidden",
      ),
    );
  });

  it("keeps the loaded rows when page two fails, and offers a retry", async () => {
    net.pages.set("", {
      items: [issueRow],
      next_cursor: "broken",
      total: 2,
      unavailable: 0,
    });
    renderSection();
    await screen.findByRole("link", { name: "Saga #1" });
    await act(async () => {
      intersect();
    });
    expect(await screen.findByText("Couldn’t load more.")).toBeTruthy();
    expect(screen.getByRole("link", { name: "Saga #1" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("labels every rail", () => {
    expect(hiddenFromLabel(["on_deck", "random_picks", "similar_series"])).toBe(
      "On deck, Random picks, Similar series",
    );
  });
});
