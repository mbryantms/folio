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

  it("on-screen pills are tab stops; untouched off-screen pills are not", async () => {
    renderStrip("double");
    // Cover solo → pages 1+2 (indices 1,2) are the visible pair.
    const onScreen = await screen.findByRole("button", {
      name: /^Page 2 pairing:/,
    });
    const offScreen = screen.getByRole("button", { name: /^Page 5 pairing:/ });
    expect(onScreen.getAttribute("tabindex")).toBe("0");
    expect(offScreen.getAttribute("tabindex")).toBe("-1");
    // Hover-revealed on a fine pointer (jsdom's matchMedia stub says
    // nothing is coarse).
    expect(offScreen.className).toMatch(/\bopacity-0\b/);
    expect(offScreen.className).toMatch(/group-hover\/thumb:opacity-100/);
  });

  // Audit N-R11: the pill was hover-only and tabbable only on the current
  // page, so touch and keyboard users could change only that page's pairing.
  it("a focused thumb puts its own pill next in the tab order", async () => {
    renderStrip("double");
    const thumb = await screen.findByRole("button", { name: "Jump to page 5" });
    const pill = screen.getByRole("button", { name: /^Page 5 pairing:/ });
    expect(pill.getAttribute("tabindex")).toBe("-1");

    act(() => thumb.focus());
    expect(pill.getAttribute("tabindex")).toBe("0");
    // The pill follows its thumb in DOM order so Tab lands on it.
    expect(
      thumb.compareDocumentPosition(pill) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // Focus moving onto the pill keeps it in the tab order…
    act(() => pill.focus());
    expect(pill.getAttribute("tabindex")).toBe("0");
    // …and leaving the thumb entirely drops it again.
    const other = screen.getByRole("button", { name: "Jump to page 6" });
    act(() => other.focus());
    expect(pill.getAttribute("tabindex")).toBe("-1");
    expect(
      screen.getByRole("button", { name: /^Page 6 pairing:/ }).tabIndex,
    ).toBe(0);
  });

  it("renders a pill for every thumb, including ones outside the loaded window", async () => {
    // 60 pages with the default 0..48 load window: page 60's thumbnail is
    // not loaded, but its pill exists so the override stays reachable.
    useReaderStore.setState({ totalPages: 60 } as never);
    const qc = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={qc}>
        <PageStrip
          issueId="issue-1"
          totalPages={60}
          currentPage={1}
          direction="ltr"
          pages={[]}
          urlVersion={null}
        />
      </QueryClientProvider>,
    );
    useReaderStore.setState({ viewMode: "double" } as never);
    const pill = await screen.findByRole("button", {
      name: /^Page 60 pairing:/,
    });
    expect(pill.getAttribute("tabindex")).toBe("-1");
    expect(screen.queryAllByRole("img")).toHaveLength(0);
  });

  it("an overridden page's pill stays shown and tabbable off-screen", async () => {
    renderStrip("double");
    const pill = await screen.findByRole("button", {
      name: /^Page 5 pairing: automatic pairing\./,
    });
    await act(async () => {
      fireEvent.click(pill);
    });
    const spread = await screen.findByRole("button", {
      name: /^Page 5 pairing: forced spread/,
    });
    expect(spread.getAttribute("tabindex")).toBe("0");
    expect(spread.className).toMatch(/\bopacity-100\b/);
    expect(spread.className).not.toMatch(/\bopacity-0\b/);
  });

  it("shows every pill on a coarse pointer, with the touch tap area", async () => {
    const matchMedia = window.matchMedia;
    window.matchMedia = (query: string) =>
      ({
        matches: query === "(pointer: coarse)",
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }) as MediaQueryList;
    try {
      renderStrip("double");
      const offScreen = await screen.findByRole("button", {
        name: /^Page 5 pairing:/,
      });
      expect(offScreen.className).toMatch(/\bopacity-100\b/);
      expect(offScreen.className).not.toMatch(/\bopacity-0\b/);
      // Visible size unchanged; the 44px target is the `folio-tap` ::after.
      expect(offScreen.className).toMatch(/\bfolio-tap\b/);
      expect(offScreen.className).toMatch(/\btext-xs\b/);
      // Still only the on-screen pair is in the tab order.
      expect(offScreen.getAttribute("tabindex")).toBe("-1");
    } finally {
      window.matchMedia = matchMedia;
    }
  });

  it("names the current pairing and the next one for assistive tech", async () => {
    renderStrip("double");
    const pill = await screen.findByRole("button", {
      name: "Page 2 pairing: automatic pairing. Change to forced spread (shown alone).",
    });
    expect(pill.getAttribute("data-spread-mode")).toBe("auto");
    expect(pill.textContent).toBe("Auto");
  });
});
