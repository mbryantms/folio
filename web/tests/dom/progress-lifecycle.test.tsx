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
