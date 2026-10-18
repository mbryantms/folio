// @vitest-environment jsdom
/**
 * `useReadingSession` must not let the previous issue's page — what the
 * singleton reader store still holds on a new issue's first render — seed
 * the session envelope. On production it landed as `end_page` (clamped to
 * the new issue's last page) on a session that had read seven pages.
 */
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const api = vi.hoisted(() => ({ send: vi.fn() }));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: api.send,
  getCsrfToken: () => "csrf",
}));
vi.mock("next/navigation", () => ({ usePathname: () => "/read/s/b" }));

import { useReadingSession } from "@/lib/reader/session";

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(new Response(null, { status: 204 })),
  );
  api.send.mockResolvedValue(new Response("{}", { status: 200 }));
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

it("seeds the envelope from the first hydrated page, not the stale one", async () => {
  vi.useFakeTimers();
  const client = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { rerender, unmount } = renderHook(
    ({ page, ready }: { page: number; ready: boolean }) =>
      useReadingSession({
        issueId: "b",
        totalPages: 22,
        currentPage: page,
        viewMode: "single",
        trackingEnabled: true,
        minActiveMs: 0,
        minPages: 0,
        ready,
      }),
    // Mounted while the store still shows the previous issue's page 31.
    { wrapper, initialProps: { page: 31, ready: false } },
  );
  // Hydrated at the resume page, then two real page turns.
  rerender({ page: 0, ready: true });
  await act(async () => {
    vi.advanceTimersByTime(2_000);
  });
  rerender({ page: 1, ready: true });
  await act(async () => {
    vi.advanceTimersByTime(2_000);
  });
  rerender({ page: 2, ready: true });
  await act(async () => {
    vi.advanceTimersByTime(2_000);
  });
  await act(async () => {
    window.dispatchEvent(new Event("pagehide"));
  });
  const posted = api.send.mock.calls.find(
    (c) => typeof c[1]?.body === "string",
  );
  expect(posted).toBeDefined();
  const body = JSON.parse(posted![1].body as string);
  expect(body).toMatchObject({
    issue_id: "b",
    start_page: 0,
    end_page: 2,
    page_turns: 2,
  });
  unmount();
});
