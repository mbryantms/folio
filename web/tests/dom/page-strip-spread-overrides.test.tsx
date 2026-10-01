// @vitest-environment jsdom
/**
 * WP-4.3 — the page strip's per-page spread-mode pill. Double view only;
 * a click cycles auto → spread → single and PUTs the whole override set
 * to `/api/me/issues/{id}/page-overrides`, re-rendering from the
 * optimistic cache write.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
  }),
}));

import { PageStrip } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/PageStrip";
import { useReaderStore } from "@/lib/reader/store";

type Call = { url: string; method: string; body: unknown };
let calls: Call[] = [];

function jsonResponse(body: unknown) {
  const text = JSON.stringify(body);
  return {
    ok: true,
    status: 200,
    headers: new Headers({ "content-type": "application/json" }),
    json: async () => body,
    text: async () => text,
  };
}

beforeEach(() => {
  calls = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = init?.method ?? "GET";
      const body = init?.body ? JSON.parse(String(init.body)) : undefined;
      calls.push({ url, method, body });
      if (url.includes("/page-overrides")) {
        if (method === "PUT") {
          return jsonResponse({
            issue_id: "issue-1",
            updated_at: "2026-09-30T00:00:00Z",
            ...(body as object),
          });
        }
        return jsonResponse({
          issue_id: "issue-1",
          shift_pairing: false,
          spread_pages: [],
          single_pages: [],
          updated_at: null,
        });
      }
      return jsonResponse({ items: [] });
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

function renderStrip(viewMode: "single" | "double") {
  useReaderStore.setState({
    issueId: "issue-1",
    currentPage: 1,
    totalPages: 6,
    viewMode,
    coverSolo: true,
    pageStripVisible: true,
    markersHidden: false,
  } as never);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <PageStrip
        issueId="issue-1"
        totalPages={6}
        currentPage={1}
        direction="ltr"
        pages={[]}
        urlVersion={null}
      />
    </QueryClientProvider>,
  );
}

describe("PageStrip spread-mode pill (jsdom)", () => {
  it("is absent outside double-page view", async () => {
    renderStrip("single");
    await screen.findByRole("button", { name: "Jump to page 3" });
    expect(screen.queryByRole("button", { name: /pairing:/ })).toBeNull();
  });

  it("cycles a page auto → spread → single and saves each step", async () => {
    renderStrip("double");
    const pill = await screen.findByRole("button", {
      name: /^Page 4 pairing: automatic pairing\./,
    });
    expect(pill.textContent).toBe("Auto");

    await act(async () => {
      fireEvent.click(pill);
    });
    const spread = await screen.findByRole("button", {
      name: /^Page 4 pairing: forced spread/,
    });
    expect(spread.textContent).toBe("Spread");
    const put1 = calls.find((c) => c.method === "PUT");
    expect(put1?.url).toContain("/api/me/issues/issue-1/page-overrides");
    expect(put1?.body).toEqual({
      shift_pairing: false,
      spread_pages: [3],
      single_pages: [],
    });

    await act(async () => {
      fireEvent.click(spread);
    });
    const single = await screen.findByRole("button", {
      name: /^Page 4 pairing: forced single/,
    });
    expect(single.textContent).toBe("Single");
    const puts = calls.filter((c) => c.method === "PUT");
    expect(puts.at(-1)?.body).toEqual({
      shift_pairing: false,
      spread_pages: [],
      single_pages: [3],
    });
  });

  it("only the on-screen pages' pills are tab stops", async () => {
    renderStrip("double");
    // Cover solo → pages 1+2 (indices 1,2) are the visible pair.
    const onScreen = await screen.findByRole("button", {
      name: /^Page 2 pairing:/,
    });
    const offScreen = screen.getByRole("button", { name: /^Page 5 pairing:/ });
    expect(onScreen.getAttribute("tabindex")).toBe("0");
    expect(offScreen.getAttribute("tabindex")).toBe("-1");
  });
});
