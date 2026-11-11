// @vitest-environment jsdom
/**
 * <RunsTab> — WP-8.3. The admin metadata Runs tab used `useQuery` on a
 * `next_cursor` response and stopped at the first 25 runs. It is now an
 * infinite query: when the sentinel intersects, the next page is fetched
 * with the previous page's cursor as `before=`, rows accumulate, and the
 * sentinel disappears once `next_cursor` is null. Filter pills restart the
 * walk from page one with the server-side param set.
 *
 * Real hook + real QueryClient; only the network (`apiFetch`) and the
 * IntersectionObserver are faked.
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

import type { RunRow, RunsListResp } from "@/lib/api/types";

const net = vi.hoisted(() => ({
  urls: [] as string[],
  pages: new Map<string, RunsListResp>(),
  broken: new Set<string>(),
}));

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (path: string) => {
    net.urls.push(path);
    const url = new URL(path, "http://test");
    const key = `${url.searchParams.get("status") ?? ""}|${url.searchParams.get("before") ?? ""}`;
    if (net.broken.has(key)) {
      return new Response(
        JSON.stringify({ error: { code: "internal", message: "internal" } }),
        { status: 500, headers: { "Content-Type": "application/json" } },
      );
    }
    const body = net.pages.get(key) ?? { runs: [], next_cursor: null };
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }),
  getCsrfToken: () => "csrf",
}));

// Controllable IntersectionObserver: `intersect()` fires every live
// observer's callback as if its target scrolled into view.
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

import { RunsTab } from "@/components/admin/metadata/RunsTab";

function run(i: number, status = "completed"): RunRow {
  return {
    id: `run-${i}`,
    scope: "series",
    scope_entity_id: null,
    library_id: null,
    trigger_kind: `trigger-${i}`,
    providers: ["metron"],
    status,
    started_at: new Date(Date.UTC(2026, 8, 1, 0, 0, 100 - i)).toISOString(),
    finished_at: null,
    items_total: 0,
    items_matched_high: 0,
    items_matched_medium: 0,
    items_matched_low: 0,
    items_applied: 0,
    items_skipped: 0,
    error_summary: null,
  };
}
const range = (from: number, to: number) =>
  Array.from({ length: to - from }, (_, k) => run(from + k));

function renderTab() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <RunsTab />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  net.urls.length = 0;
  net.pages.clear();
  net.broken.clear();
  io.live.clear();
});

describe("RunsTab pagination", () => {
  it("walks past the first 25 runs via the sentinel and stops at the end", async () => {
    net.pages.set("|", { runs: range(0, 25), next_cursor: "c1" });
    net.pages.set("|c1", { runs: range(25, 50), next_cursor: "c2" });
    net.pages.set("|c2", { runs: range(50, 60), next_cursor: null });
    renderTab();

    await screen.findByText(/trigger-24\b/);
    expect(screen.queryByText(/trigger-25\b/)).toBeNull();
    expect(screen.getByTestId("runs-sentinel").className).not.toContain(
      "hidden",
    );

    act(() => intersect());
    await screen.findByText(/trigger-49\b/);
    act(() => intersect());
    await screen.findByText(/trigger-59\b/);

    // Every run rendered exactly once, newest first.
    const rows = screen.getAllByText(/trigger-\d+/);
    expect(rows).toHaveLength(60);
    expect(rows[0]?.textContent).toContain("trigger-0");
    expect(rows[59]?.textContent).toContain("trigger-59");

    // Cursors went back as `before=`; nothing fetched past the last page.
    expect(net.urls).toEqual([
      "/admin/metadata/runs",
      "/admin/metadata/runs?before=c1",
      "/admin/metadata/runs?before=c2",
    ]);
    await waitFor(() =>
      expect(screen.getByTestId("runs-sentinel").className).toContain("hidden"),
    );
    act(() => intersect());
    expect(net.urls).toHaveLength(3);
  });

  it("a failed next page keeps the rows, stops fetching and offers Retry", async () => {
    net.pages.set("|", { runs: range(0, 25), next_cursor: "c1" });
    net.pages.set("|c1", { runs: range(25, 30), next_cursor: null });
    net.broken.add("|c1");
    renderTab();
    await screen.findByText(/trigger-24\b/);

    act(() => intersect());
    expect(await screen.findByText("Couldn't load more.")).toBeTruthy();
    expect(screen.getByText(/trigger-24\b/)).toBeTruthy();
    const pageTwo = () => net.urls.filter((u) => u.endsWith("before=c1"));
    expect(pageTwo()).toHaveLength(1);

    // Parked: the sentinel coming into view again does not refetch.
    act(() => intersect());
    expect(pageTwo()).toHaveLength(1);

    net.broken.delete("|c1");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByText(/trigger-29\b/);
    expect(pageTwo()).toHaveLength(2);
    expect(screen.queryByText("Couldn't load more.")).toBeNull();
  });

  it("filter pills restart from page one with a server-side param", async () => {
    net.pages.set("|", { runs: range(0, 25), next_cursor: "c1" });
    net.pages.set("failed|", {
      runs: [run(900, "failed")],
      next_cursor: null,
    });
    renderTab();
    await screen.findByText(/trigger-0\b/);

    fireEvent.click(screen.getByRole("button", { name: "Failed" }));
    await screen.findByText(/trigger-900\b/);
    expect(screen.queryByText(/trigger-0\b/)).toBeNull();
    expect(net.urls.at(-1)).toBe("/admin/metadata/runs?status=failed");
  });
});
