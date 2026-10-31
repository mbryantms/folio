// @vitest-environment jsdom
/**
 * Spread overrides offline (audit N-R7 / N-R11): `useIssuePageOverrides`
 * fires once and never retries while `navigator.onLine` is false, and
 * `useSpreadOverrides` pairs the pages with the download record's stored
 * copy while that one request is pending or after it fails — the live
 * answer wins when the network is back.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import {
  onlineManager,
  QueryClient,
  QueryClientProvider,
} from "@tanstack/react-query";

import { useIssuePageOverrides } from "@/lib/api/queries";
import type { PageOverridesView } from "@/lib/api/types";
import { useSpreadOverrides } from "@/lib/reader/use-spread-overrides";

const stored: PageOverridesView = {
  issue_id: "i1",
  shift_pairing: true,
  spread_pages: [6],
  single_pages: [],
  updated_at: "2026-10-01T00:00:00Z",
};
const live: PageOverridesView = {
  ...stored,
  shift_pairing: false,
  single_pages: [3],
  updated_at: "2026-10-02T00:00:00Z",
};
const asSpread = (v: PageOverridesView) => ({
  shift_pairing: v.shift_pairing,
  spread_pages: v.spread_pages,
  single_pages: v.single_pages,
});

let online = true;
let fetchSpy: ReturnType<typeof vi.fn>;

function wrapper() {
  // Default retry policy on purpose: the hook under test is what turns
  // retries off offline.
  const qc = new QueryClient();
  function Wrapper({ children }: { children: React.ReactNode }) {
    return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  }
  return Wrapper;
}

const useBoth = () => ({
  query: useIssuePageOverrides("i1"),
  spread: useSpreadOverrides("i1", null, stored),
});

beforeEach(() => {
  online = false;
  fetchSpy = vi.fn(async () => {
    throw new TypeError("Failed to fetch");
  });
  vi.stubGlobal("fetch", fetchSpy);
  Object.defineProperty(window.navigator, "onLine", {
    configurable: true,
    get: () => online,
  });
});
afterEach(() => {
  vi.unstubAllGlobals();
  onlineManager.setOnline(true);
  Object.defineProperty(window.navigator, "onLine", {
    configurable: true,
    get: () => true,
  });
});

describe("useIssuePageOverrides offline", () => {
  it("fires the request once and settles without retrying", async () => {
    const { result } = renderHook(() => useIssuePageOverrides("i1"), {
      wrapper: wrapper(),
    });
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(String(fetchSpy.mock.calls[0]?.[0])).toBe(
      "/api/me/issues/i1/page-overrides",
    );
  });

  it("is not parked when TanStack already knows the browser is offline", async () => {
    onlineManager.setOnline(false);
    const { result } = renderHook(() => useIssuePageOverrides("i1"), {
      wrapper: wrapper(),
    });
    // The default `online` network mode would leave this `paused`
    // forever; `offlineFirst` fires once and reports the failure.
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.fetchStatus).toBe("idle");
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });
});

describe("useSpreadOverrides with a stored copy", () => {
  it("pairs with the stored copy while pending and after the fetch rejects", async () => {
    const { result } = renderHook(useBoth, { wrapper: wrapper() });
    // No automatic-pairing flash before the request settles.
    expect(result.current.spread).toEqual(asSpread(stored));
    await waitFor(() => expect(result.current.query.isError).toBe(true));
    expect(result.current.spread).toEqual(asSpread(stored));
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });

  it("the live answer wins over the stored copy when the network is back", async () => {
    online = true;
    fetchSpy.mockImplementation(
      async () =>
        new Response(JSON.stringify(live), {
          headers: { "content-type": "application/json" },
        }),
    );
    const { result } = renderHook(useBoth, { wrapper: wrapper() });
    expect(result.current.spread).toEqual(asSpread(stored));
    await waitFor(() => expect(result.current.query.isSuccess).toBe(true));
    expect(result.current.spread).toEqual(asSpread(live));
  });

  it("stays on automatic pairing with no stored copy and a failed fetch", async () => {
    const { result } = renderHook(
      () => ({
        query: useIssuePageOverrides("i1"),
        spread: useSpreadOverrides("i1", null, null),
      }),
      { wrapper: wrapper() },
    );
    await waitFor(() => expect(result.current.query.isError).toBe(true));
    expect(result.current.spread).toBeNull();
  });
});
