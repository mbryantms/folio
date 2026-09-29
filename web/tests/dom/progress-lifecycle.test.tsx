// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
const api = vi.hoisted(() => ({ send: vi.fn() }));
vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: api.send,
  getCsrfToken: () => "csrf",
}));
vi.mock("@/lib/api/mutations", () => ({ invalidateRails: vi.fn() }));
import { useReaderProgressWrite } from "@/lib/reader/use-progress-write";
afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});
it("flushes on hidden, retains a failed POST, and retries on online", async () => {
  vi.useFakeTimers();
  api.send
    .mockResolvedValueOnce(new Response("error", { status: 503 }))
    .mockResolvedValue(new Response(null, { status: 204 }));
  const client = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { unmount } = renderHook(
    () =>
      useReaderProgressWrite({
        issueId: "a",
        currentPage: 4,
        initialPage: 0,
        totalPages: 20,
        incognito: false,
      }),
    { wrapper },
  );
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    value: "hidden",
  });
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(api.send).toHaveBeenCalledTimes(1);
  expect(api.send.mock.calls[0]![1].keepalive).toBe(true);
  await act(async () => {
    window.dispatchEvent(new Event("online"));
  });
  expect(api.send).toHaveBeenCalledTimes(2);
  await act(async () => {
    window.dispatchEvent(new Event("pagehide"));
  });
  expect(api.send).toHaveBeenCalledTimes(2);
  unmount();
});

it("opens a new run on restart, then echoes the run the server returns", async () => {
  vi.useFakeTimers();
  api.send
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ run: 3 }), { status: 200 }),
    )
    .mockResolvedValue(
      new Response(JSON.stringify({ run: 3 }), { status: 200 }),
    );
  const client = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { rerender, unmount } = renderHook(
    ({ page }: { page: number }) =>
      useReaderProgressWrite({
        issueId: "a",
        currentPage: page,
        initialPage: 0,
        initialRun: 2,
        restartRun: true,
        totalPages: 20,
        incognito: false,
      }),
    { wrapper, initialProps: { page: 1 } },
  );
  await act(async () => {
    vi.advanceTimersByTime(400);
  });
  expect(api.send).toHaveBeenCalledTimes(1);
  const first = JSON.parse(api.send.mock.calls[0]![1].body as string);
  expect(first).toMatchObject({ issue_id: "a", page: 1, restart: true });
  expect(first.run).toBeUndefined();

  rerender({ page: 2 });
  await act(async () => {
    vi.advanceTimersByTime(400);
  });
  expect(api.send).toHaveBeenCalledTimes(2);
  const second = JSON.parse(api.send.mock.calls[1]![1].body as string);
  expect(second).toMatchObject({ issue_id: "a", page: 2, run: 3 });
  expect(second.restart).toBeUndefined();
  unmount();
});

it("tags per-page writes with the saved run when not restarting", async () => {
  vi.useFakeTimers();
  api.send.mockResolvedValue(new Response(null, { status: 204 }));
  const client = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const { unmount } = renderHook(
    () =>
      useReaderProgressWrite({
        issueId: "a",
        currentPage: 5,
        initialPage: 3,
        initialRun: 1,
        totalPages: 20,
        incognito: false,
      }),
    { wrapper },
  );
  await act(async () => {
    vi.advanceTimersByTime(400);
  });
  const body = JSON.parse(api.send.mock.calls[0]![1].body as string);
  expect(body).toMatchObject({ page: 5, run: 1 });
  expect(body.restart).toBeUndefined();
  unmount();
});
