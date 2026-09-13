// @vitest-environment jsdom
/**
 * Hydrated render tests for the reader chrome: the page-jump control, the
 * settings popover and the marker menu, wired to the real zustand store.
 * These are the interactions a React / Radix / TanStack bump is most
 * likely to break silently — the store logic itself is covered in
 * tests/reader/store-chrome.test.ts.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const push = vi.fn();
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push, refresh: vi.fn() }),
}));
vi.mock("next/link", () => ({
  default: ({ children, href }: { children: React.ReactNode; href: string }) =>
    React.createElement("a", { href }, children),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
  }),
}));

import { ReaderChrome } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/ReaderChrome";
import { useReaderStore } from "@/lib/reader/store";

function resetStore(
  overrides: Partial<ReturnType<typeof useReaderStore.getState>> = {},
) {
  useReaderStore.setState({
    issueId: "issue-1",
    seriesId: "series-1",
    currentPage: 0,
    totalPages: 20,
    chromeVisible: true,
    chromeAutoHide: false,
    chromePinned: false,
    pendingMarker: null,
    markerMode: "off",
    ...overrides,
  } as never);
}

function renderChrome(
  props: Partial<React.ComponentProps<typeof ReaderChrome>> = {},
) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ReaderChrome
        seriesId="series-1"
        issueId="issue-1"
        exitUrl="/series/s/issue-1"
        totalPages={20}
        progressCurrent={0}
        progressTotal={20}
        {...props}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  resetStore();
  // The bookmark/favorite buttons query /api/me/issues/:id/markers on mount.
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({
      ok: true,
      status: 200,
      headers: new Headers({ "content-type": "application/json" }),
      json: async () => ({ items: [] }),
      text: async () => '{"items":[]}',
    })),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
  push.mockReset();
});

describe("ReaderChrome (jsdom)", () => {
  it("mounts open when chromeVisible is set and exits via the router", async () => {
    renderChrome();
    const exit = await screen.findByRole("button", { name: "Exit reader" });
    // The chrome flips data-state after its first animation frame.
    await waitFor(() =>
      expect(exit.closest("header")?.getAttribute("data-state")).toBe("open"),
    );
    fireEvent.click(exit);
    expect(push).toHaveBeenCalledWith("/series/s/issue-1");
  });

  it("page indicator: click → number input → Enter commits a clamped page to the store", async () => {
    renderChrome();
    const indicator = await screen.findByRole("button", {
      name: "Page 1 of 20; click to jump",
    });
    fireEvent.click(indicator);
    const input = (await screen.findByLabelText(
      "Jump to page (1–20)",
    )) as HTMLInputElement;
    expect(useReaderStore.getState().chromePinned).toBe(true);

    fireEvent.change(input, { target: { value: "7" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(useReaderStore.getState().currentPage).toBe(6);
    expect(useReaderStore.getState().chromePinned).toBe(false);

    // Escape cancels without touching the page.
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Page 7 of 20; click to jump",
      }),
    );
    const again = (await screen.findByLabelText(
      "Jump to page (1–20)",
    )) as HTMLInputElement;
    fireEvent.change(again, { target: { value: "3" } });
    fireEvent.keyDown(again, { key: "Escape" });
    expect(useReaderStore.getState().currentPage).toBe(6);
  });

  it("settings gear opens the reader-settings popover and pins the chrome", async () => {
    renderChrome();
    fireEvent.click(
      await screen.findByRole("button", { name: "Reader settings" }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toBeTruthy();
    expect(screen.getByRole("group", { name: "View mode" })).toBeTruthy();
    expect(
      screen.getByRole("group", { name: "Reading direction" }),
    ).toBeTruthy();
    expect(useReaderStore.getState().chromePinned).toBe(true);
  });

  it("marker menu → Add note seeds a pending note on the current page", async () => {
    resetStore({ currentPage: 4 });
    renderChrome();
    const trigger = await screen.findByRole("button", { name: "Marker tools" });
    await act(async () => {
      fireEvent.keyDown(trigger, { key: "Enter" });
    });
    const item = await screen.findByRole("menuitem", { name: /add note/i });
    await act(async () => {
      fireEvent.click(item);
    });
    expect(useReaderStore.getState().pendingMarker).toEqual({
      kind: "note",
      page_index: 4,
      region: null,
      selection: null,
      body: "",
      is_favorite: false,
      tags: [],
    });
  });

  it("renders the incognito chip only when asked", async () => {
    const { unmount } = renderChrome({ incognito: true });
    expect(await screen.findByText("Incognito")).toBeTruthy();
    unmount();
    renderChrome();
    await screen.findByRole("button", { name: "Exit reader" });
    expect(screen.queryByText("Incognito")).toBeNull();
  });
});
