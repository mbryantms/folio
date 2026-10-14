// @vitest-environment jsdom
/**
 * Post-audit T1.4: in double-page view the resume anchor is the first
 * page of the spread group, so a final pair `[n-2, n-1]` used to never
 * send `finished: true`. The hook now keys `finished` on the furthest
 * visible page while persisting the anchor as the resume page.
 */
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";

const api = vi.hoisted(() => ({ send: vi.fn() }));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: api.send,
  getCsrfToken: () => "csrf",
}));
vi.mock("@/lib/api/mutations/rails", () => ({ invalidateRails: vi.fn() }));

import { useReaderProgressWrite } from "@/lib/reader/use-progress-write";

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

function wrapper() {
  const client = new QueryClient();
  function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  }
  return Wrapper;
}

async function lastBody(): Promise<Record<string, unknown>> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(500);
  });
  const calls = api.send.mock.calls;
  expect(calls.length).toBeGreaterThan(0);
  return JSON.parse(calls[calls.length - 1]![1].body as string);
}

it("a final spread pair finishes the issue while resuming on its first page", async () => {
  vi.useFakeTimers();
  api.send.mockResolvedValue(new Response(null, { status: 204 }));
  const { unmount } = renderHook(
    () =>
      useReaderProgressWrite({
        issueId: "a",
        currentPage: 18,
        furthestPage: 19,
        initialPage: 0,
        totalPages: 20,
        incognito: false,
      }),
    { wrapper: wrapper() },
  );
  const body = await lastBody();
  expect(body.page).toBe(18);
  expect(body.finished).toBe(true);
  unmount();
});

it("a mid-issue pair does not finish the issue", async () => {
  vi.useFakeTimers();
  api.send.mockResolvedValue(new Response(null, { status: 204 }));
  const { unmount } = renderHook(
    () =>
      useReaderProgressWrite({
        issueId: "a",
        currentPage: 16,
        furthestPage: 17,
        initialPage: 0,
        totalPages: 20,
        incognito: false,
      }),
    { wrapper: wrapper() },
  );
  const body = await lastBody();
  expect(body.page).toBe(16);
  expect(body.finished).toBeUndefined();
  unmount();
});

it("a solo final page still finishes without furthestPage", async () => {
  vi.useFakeTimers();
  api.send.mockResolvedValue(new Response(null, { status: 204 }));
  const { unmount } = renderHook(
    () =>
      useReaderProgressWrite({
        issueId: "a",
        currentPage: 19,
        initialPage: 0,
        totalPages: 20,
        incognito: false,
      }),
    { wrapper: wrapper() },
  );
  const body = await lastBody();
  expect(body.page).toBe(19);
  expect(body.finished).toBe(true);
  unmount();
});
