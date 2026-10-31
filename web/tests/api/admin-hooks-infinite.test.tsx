// @vitest-environment jsdom
/**
 * The three admin hooks the 2026-11 audit (N-O7) found on `useQuery` over a
 * `next_cursor` response — `useScanRuns`, `useScanBatches`,
 * `useAdminHealthIssues` — are infinite queries now. Real hooks + a real
 * QueryClient; only the network (`apiFetch`) is faked. For each hook: when
 * the first page carries `next_cursor`, `hasNextPage` is true and
 * `fetchNextPage` requests the same endpoint with `cursor=<that value>`
 * (the filter params intact); when it is `null`, the walk stops and no
 * second request is made.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import * as React from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const net = vi.hoisted(() => ({
  urls: [] as string[],
  /** Response body keyed by the request's `cursor` param ("" = first page). */
  pages: new Map<string, unknown>(),
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

import {
  useAdminHealthIssues,
  useScanBatches,
  useScanRuns,
} from "@/lib/api/queries";

function wrapper() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  };
}

function urlsFor(path: string) {
  return net.urls.filter((u) => u.startsWith(path));
}

function cursorOf(url: string) {
  return new URL(url, "http://test").searchParams.get("cursor");
}

type Fixture = {
  name: string;
  path: string;
  /** Call the hook with a filter that must survive onto page two. */
  useHook: () => {
    data: { pages: { items: unknown[] }[] } | undefined;
    hasNextPage: boolean;
    isFetchingNextPage: boolean;
    fetchNextPage: () => Promise<unknown>;
  };
  filterParam: [string, string];
};

const FIXTURES: Fixture[] = [
  {
    name: "useScanRuns",
    path: "/libraries/lib-1/scan-runs",
    useHook: () => useScanRuns("lib-1", { kind: "series" }),
    filterParam: ["kind", "series"],
  },
  {
    name: "useScanBatches",
    path: "/admin/scan-batches",
    useHook: () => useScanBatches("complete"),
    filterParam: ["state", "complete"],
  },
  {
    name: "useAdminHealthIssues",
    path: "/admin/health-issues",
    useHook: () => useAdminHealthIssues({ severity: "error", limit: 50 }),
    filterParam: ["severity", "error"],
  },
];

describe.each(FIXTURES)("$name walks next_cursor", (fx) => {
  beforeEach(() => {
    net.urls.length = 0;
    net.pages.clear();
  });

  it("requests page two with the first page's cursor and the same filter", async () => {
    net.pages.set("", { items: [{ id: "a" }], next_cursor: "c-2" });
    net.pages.set("c-2", { items: [{ id: "b" }], next_cursor: null });

    const { result } = renderHook(fx.useHook, { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.hasNextPage).toBe(true);

    await act(async () => {
      await result.current.fetchNextPage();
    });
    await waitFor(() => expect(result.current.data?.pages).toHaveLength(2));

    const urls = urlsFor(fx.path);
    expect(urls).toHaveLength(2);
    expect(cursorOf(urls[0]!)).toBeNull();
    expect(cursorOf(urls[1]!)).toBe("c-2");
    // Filter chips are server params and ride along onto every page.
    for (const u of urls) {
      expect(
        new URL(u, "http://test").searchParams.get(fx.filterParam[0]),
      ).toBe(fx.filterParam[1]);
    }
    expect(result.current.data?.pages.flatMap((p) => p.items)).toEqual([
      { id: "a" },
      { id: "b" },
    ]);
    expect(result.current.hasNextPage).toBe(false);
  });

  it("stops when next_cursor is null — no second request", async () => {
    net.pages.set("", { items: [{ id: "a" }], next_cursor: null });

    const { result } = renderHook(fx.useHook, { wrapper: wrapper() });
    await waitFor(() => expect(result.current.data).toBeDefined());

    expect(result.current.hasNextPage).toBe(false);
    await act(async () => {
      // A stray call must be a no-op, not a request with `cursor=`.
      await result.current.fetchNextPage();
    });
    expect(urlsFor(fx.path)).toHaveLength(1);
  });
});
