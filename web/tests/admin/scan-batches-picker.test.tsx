// @vitest-environment jsdom
/**
 * <BatchPicker> on the admin Scan dashboard — audit 2026-11 N-O7. The
 * picker used `useQuery` on `GET /admin/scan-batches` and showed only the
 * first page of "Scan all" batches. It is now an infinite query: when the
 * IntersectionObserver sentinel intersects, the next page is requested with
 * the previous page's `next_cursor`, rows accumulate, and the sentinel
 * hides once `next_cursor` is null.
 *
 * Real hook + real QueryClient; only the network (`apiFetch`) and the
 * IntersectionObserver are faked (same harness as metadata-runs-tab).
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ScanBatchView } from "@/lib/api/types";

const net = vi.hoisted(() => ({
  urls: [] as string[],
  pages: new Map<
    string,
    { items: ScanBatchView[]; next_cursor: string | null }
  >(),
}));

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (path: string) => {
    net.urls.push(path);
    const url = new URL(path, "http://test");
    const body = net.pages.get(url.searchParams.get("cursor") ?? "") ?? {
      items: [],
      next_cursor: null,
    };
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

import { BatchPicker } from "@/components/admin/library/ScanDashboardClient";

function batch(i: number): ScanBatchView {
  return {
    id: `batch-${i}`,
    kind: "scan_all",
    state: "complete",
    force: false,
    library_count: i,
    started_at: `2026-06-0${i}T00:00:00Z`,
    ended_at: null,
    actor_id: null,
    runs: { queued: 0, running: 0, complete: i, failed: 0, cancelled: 0 },
  };
}

function renderPicker() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={qc}>
      <BatchPicker />
    </QueryClientProvider>,
  );
}

describe("<BatchPicker> paging", () => {
  beforeEach(() => {
    net.urls.length = 0;
    net.pages.clear();
    io.live.clear();
  });

  it("fetches the next page when the sentinel intersects, then hides it", async () => {
    net.pages.set("", { items: [batch(1), batch(2)], next_cursor: "c-2" });
    net.pages.set("c-2", { items: [batch(3)], next_cursor: null });

    renderPicker();
    await waitFor(() =>
      expect(screen.getAllByRole("link", { name: /librar/ })).toHaveLength(2),
    );
    const sentinel = screen.getByTestId("infinite-sentinel");
    expect(sentinel.className).not.toContain("hidden");
    expect(net.urls).toHaveLength(1);

    await act(async () => {
      intersect();
    });

    await waitFor(() =>
      expect(screen.getAllByRole("link", { name: /librar/ })).toHaveLength(3),
    );
    expect(net.urls).toHaveLength(2);
    expect(
      new URL(net.urls[1]!, "http://test").searchParams.get("cursor"),
    ).toBe("c-2");
    // The walk is over: the sentinel hides and a further intersection is a
    // no-op rather than a request with a stale cursor.
    await waitFor(() =>
      expect(screen.getByTestId("infinite-sentinel").className).toContain(
        "hidden",
      ),
    );
    await act(async () => {
      intersect();
    });
    expect(net.urls).toHaveLength(2);
  });

  it("renders a single page with the sentinel hidden when next_cursor is null", async () => {
    net.pages.set("", { items: [batch(1)], next_cursor: null });

    renderPicker();
    await waitFor(() =>
      expect(screen.getAllByRole("link", { name: /librar/ })).toHaveLength(1),
    );
    expect(screen.getByTestId("infinite-sentinel").className).toContain(
      "hidden",
    );
    await act(async () => {
      intersect();
    });
    expect(net.urls).toHaveLength(1);
  });
});
