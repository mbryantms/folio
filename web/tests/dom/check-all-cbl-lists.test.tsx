// @vitest-environment jsdom
/**
 * `useCheckAllCblLists`: the panel's result handling lives on the hook, so
 * the error toast's Retry fills the panel too; a check abandoned through
 * its signal sends no further round and shows no toast.
 */
import * as React from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { toast } = vi.hoisted(() => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    message: vi.fn(),
    info: vi.fn(),
  }),
}));
vi.mock("sonner", () => ({ toast }));
vi.mock("@/lib/api/auth-refresh", () => ({ apiFetch: vi.fn() }));

import { apiFetch } from "@/lib/api/auth-refresh";
import { useCheckAllCblLists } from "@/lib/api/mutations";
import type { CblBulkItemView } from "@/lib/api/types";

const fetchMock = vi.mocked(apiFetch);

function item(id: string, attempted = true): CblBulkItemView {
  return {
    id,
    name: id,
    ok: attempted,
    error: attempted ? null : "not attempted",
    applied: false,
    changed: false,
    upstream_changed: false,
    added: 0,
    removed: 0,
    reordered: 0,
    rematched: 0,
    attempted,
  };
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { mutations: { retry: 0 } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("useCheckAllCblLists", () => {
  it("delivers the result of a toast Retry after a failed first round", async () => {
    fetchMock.mockResolvedValueOnce(
      json({ error: { code: "internal", message: "boom" } }, 503),
    );
    fetchMock.mockResolvedValueOnce(
      json({ items: [item("a")], changed_count: 0, failed_count: 0 }),
    );
    const onResult = vi.fn();
    const { result } = renderHook(() => useCheckAllCblLists({ onResult }), {
      wrapper,
    });
    act(() => result.current.mutate({}));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(onResult).not.toHaveBeenCalled();
    const retry = toast.error.mock.calls[0]![1].action.onClick as () => void;
    act(() => retry());
    await waitFor(() => expect(onResult).toHaveBeenCalledTimes(1));
    expect(onResult.mock.calls[0]![0].items).toEqual([item("a")]);
    expect(toast.success).toHaveBeenCalledTimes(1);
  });

  it("stops and stays quiet once its signal is aborted", async () => {
    const controller = new AbortController();
    fetchMock.mockImplementationOnce(async () => {
      controller.abort();
      return json({
        items: [item("a"), item("b", false)],
        changed_count: 0,
        failed_count: 1,
      });
    });
    const onResult = vi.fn();
    const onSettled = vi.fn();
    const { result } = renderHook(
      () => useCheckAllCblLists({ onResult, onSettled }),
      { wrapper },
    );
    act(() => result.current.mutate({ signal: controller.signal }));
    await waitFor(() => expect(onSettled).toHaveBeenCalledTimes(1));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(onResult).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
  });
});
