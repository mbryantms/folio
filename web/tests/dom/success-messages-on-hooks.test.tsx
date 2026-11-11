// @vitest-environment jsdom
/**
 * Audit 2026-11 U-18: a mutation's success toast lives on its hook
 * (`successMessage` on `useApiMutation`), never at the call site. Each
 * hook that took over a call-site message toasts it exactly once; the
 * former call sites add no second toast.
 */
import * as React from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as Queries from "@/lib/api/queries";
import type * as QueriesReader from "@/lib/api/queries-reader";

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
vi.mock("@/lib/api/queries", async (orig) => ({
  ...(await orig<typeof Queries>()),
  useIssueMarkers: () => ({ data: { items: [] } }),
}));
vi.mock("@/lib/api/queries-reader", async (orig) => ({
  ...(await orig<typeof QueriesReader>()),
  useIssueMarkers: () => ({ data: { items: [] } }),
}));

import { apiFetch } from "@/lib/api/auth-refresh";
import {
  useCreateCollection,
  useCreateSavedView,
  useCreateSavedViewBatch,
} from "@/lib/api/mutations";
import { useCreateMarker, useUpdateMarker } from "@/lib/api/mutations/markers";
import { usePageMarkerToggle } from "@/lib/markers/use-page-marker-toggle";

const fetchMock = vi.mocked(apiFetch);

function respond(body: unknown) {
  fetchMock.mockResolvedValue(
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  );
}

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { mutations: { retry: 0 } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("useCreateMarker", () => {
  it.each([
    ["bookmark", 4, "Bookmarked page 5"],
    ["favorite", 0, "Starred page 1"],
    ["highlight", 2, "Highlight saved"],
    ["note", 2, "Note saved"],
  ] as const)("toasts a %s once", async (kind, page_index, text) => {
    respond({ id: "m-1" });
    const { result } = renderHook(() => useCreateMarker(), { wrapper });
    act(() => result.current.mutate({ issue_id: "i-1", page_index, kind }));
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(toast.success.mock.calls[0]![0]).toBe(text);
  });

  it("stays quiet when silent", async () => {
    respond({ id: "m-1" });
    const { result } = renderHook(() => useCreateMarker({ silent: true }), {
      wrapper,
    });
    act(() =>
      result.current.mutate({
        issue_id: "i-1",
        page_index: 0,
        kind: "bookmark",
      }),
    );
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(toast.success).not.toHaveBeenCalled();
  });
});

describe("useUpdateMarker", () => {
  it.each([
    [{ selection: { text: "POW", ocr_confidence: 0.9 } }, "Text captured"],
    [{ selection: null }, "Detected text cleared"],
    [{ body: "x", tags: [] }, "Saved"],
  ])("toasts %j once", async (input, text) => {
    respond({ id: "m-1" });
    const { result } = renderHook(() => useUpdateMarker("m-1", "i-1"), {
      wrapper,
    });
    act(() => result.current.mutate(input));
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(toast.success).toHaveBeenCalledOnce();
    expect(toast.success.mock.calls[0]![0]).toBe(text);
  });
});

describe("saved views and collections", () => {
  it("useCreateSavedView toasts View created once", async () => {
    respond({ id: "v-1" });
    const { result } = renderHook(() => useCreateSavedView(), { wrapper });
    act(() =>
      result.current.mutate({ kind: "series", name: "Capes" } as never),
    );
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(toast.success).toHaveBeenCalledOnce();
    expect(toast.success.mock.calls[0]![0]).toBe("View created");
  });

  it("useCreateSavedViewBatch counts the batch and links to Review", async () => {
    respond({ batch_id: "b-9", items_total: 7 });
    const { result } = renderHook(() => useCreateSavedViewBatch(), {
      wrapper,
    });
    act(() => result.current.mutate({ saved_view_id: "v-1" }));
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(toast.success).toHaveBeenCalledOnce();
    const [text, opts] = toast.success.mock.calls[0]!;
    expect(text).toBe("Searching 7 items for metadata");
    expect(opts.action.label).toBe("Review");
  });

  it("useCreateCollection names the new collection once", async () => {
    respond({ id: "c-1", name: "Capes" });
    const { result } = renderHook(() => useCreateCollection(), { wrapper });
    act(() => result.current.mutate({ name: "Capes" } as never));
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(toast.success).toHaveBeenCalledOnce();
    expect(toast.success.mock.calls[0]![0]).toBe('Collection "Capes" created');
  });
});

describe("former call sites add no second toast", () => {
  it("the reader's page bookmark toggle toasts only the hook's message", async () => {
    respond({ id: "m-1" });
    const { result } = renderHook(
      () => usePageMarkerToggle("i-1", 2, "bookmark"),
      { wrapper },
    );
    act(() => result.current.toggle({ removed: "Bookmark removed" }));
    await waitFor(() => expect(toast.success).toHaveBeenCalled());
    // Give a stray call-site toast the chance to land.
    await act(async () => {});
    expect(toast.success).toHaveBeenCalledOnce();
    expect(toast.success.mock.calls[0]![0]).toBe("Bookmarked page 3");
  });

  it("the chrome's toggle creates without a toast", async () => {
    respond({ id: "m-1" });
    const { result } = renderHook(
      () => usePageMarkerToggle("i-1", 2, "bookmark", { silentCreate: true }),
      { wrapper },
    );
    act(() => result.current.toggle({ removed: "Bookmark removed" }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    await act(async () => {});
    expect(toast.success).not.toHaveBeenCalled();
  });
});
