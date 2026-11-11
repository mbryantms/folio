// @vitest-environment jsdom
/**
 * Leaving the reader (audit U-02), through the whole `Reader`: the close
 * control goes back to the page reading started from (tracked by
 * lib/ui/back-flight), or replaces the reader with the issue page on a
 * cold load; the next / previous issue replace the reader's history entry
 * instead of pushing a new one.
 */
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const router = vi.hoisted(() => ({
  push: vi.fn(),
  replace: vi.fn(),
  back: vi.fn(),
  refresh: vi.fn(),
  prefetch: vi.fn(),
}));
vi.mock("next/navigation", () => ({
  useRouter: () => router,
  usePathname: () => "/read/saga/issue-1",
  useSearchParams: () => new URLSearchParams(),
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
    message: vi.fn(),
  }),
}));

import type { Reader } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader";
import type { IssueSummaryView, NextUpView } from "@/lib/api/types";

const EXIT_URL = "/series/saga/issues/issue-1";

function upTo(slug: string): NextUpView {
  return {
    source: "series",
    target: { id: slug, slug, series_slug: "saga" } as IssueSummaryView,
  } as NextUpView;
}

async function loadTracker(history: string[]) {
  vi.resetModules();
  const flight = await import("@/lib/ui/back-flight");
  for (const p of history) flight.noteRoute(p);
  return flight;
}

async function renderReader() {
  const { Reader: FreshReader } =
    await import("@/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const props: React.ComponentProps<typeof Reader> = {
    issueId: "issue-1",
    seriesId: "s1",
    cblSavedViewId: null,
    exitUrl: EXIT_URL,
    totalPages: 3,
    initialPage: 0,
    pages: [],
    pageUrlVersion: null,
    manga: null,
    userDefaultDirection: null,
    libraryDefaultDirection: null,
    seriesReadingDirection: null,
    userDefaultFitMode: null,
    userDefaultViewMode: "single",
    userDefaultPageStrip: false,
    userDefaultPageAnimation: "off",
    userDefaultCoverSolo: true,
    userKeybinds: {},
    activityTrackingEnabled: false,
    readingMinActiveMs: 30_000,
    readingMinPages: 3,
    readingIdleMs: 180_000,
    nextUpOverride: upTo("issue-2"),
    prevUpOverride: upTo("issue-0"),
  };
  render(
    <QueryClientProvider client={qc}>
      <FreshReader {...props} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  // jsdom has no image decoding; the prefetcher awaits it.
  HTMLImageElement.prototype.decode = () => Promise.resolve();
  localStorage.setItem("reader:firstRunSeen:v1", "1");
  vi.stubGlobal(
    "fetch",
    // Every side query (markers, spread overrides, progress) just fails:
    // the reader works without them, and only navigation is under test.
    vi.fn(async () => {
      const body = { error: { code: "not_found", message: "not found" } };
      return {
        ok: false,
        status: 404,
        headers: new Headers({ "content-type": "application/json" }),
        json: async () => body,
        text: async () => JSON.stringify(body),
      };
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
  Object.values(router).forEach((f) => f.mockReset());
});

async function clickExit() {
  const exit = await screen.findByRole(
    "button",
    { name: "Exit reader", hidden: true },
    { timeout: 5000 },
  );
  fireEvent.click(exit);
}

describe("reader exit (jsdom)", () => {
  it("the close control goes back to the page reading started from", async () => {
    await loadTracker(["/series/saga", "/read/saga/issue-1"]);
    await renderReader();
    await clickExit();
    expect(router.back).toHaveBeenCalledTimes(1);
    expect(router.push).not.toHaveBeenCalled();
    expect(router.replace).not.toHaveBeenCalled();
  });

  it("on a cold load it replaces the reader with the issue page", async () => {
    await loadTracker(["/read/saga/issue-1"]);
    await renderReader();
    await clickExit();
    expect(router.replace).toHaveBeenCalledWith(EXIT_URL);
    expect(router.back).not.toHaveBeenCalled();
    expect(router.push).not.toHaveBeenCalled();
  });

  it("Esc exits the same way", async () => {
    await loadTracker(["/", "/read/saga/issue-1"]);
    await renderReader();
    await screen.findByRole(
      "button",
      { name: "Exit reader", hidden: true },
      { timeout: 5000 },
    );
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(router.back).toHaveBeenCalledTimes(1));
  });

  it("next and previous issue replace the history entry, not push", async () => {
    const flight = await loadTracker(["/series/saga", "/read/saga/issue-1"]);
    await renderReader();
    await screen.findByRole(
      "button",
      { name: "Exit reader", hidden: true },
      { timeout: 5000 },
    );

    fireEvent.keyDown(window, { key: "N", shiftKey: true });
    await waitFor(() => expect(router.replace).toHaveBeenCalledTimes(1));
    expect(router.replace.mock.calls[0]![0]).toMatch(/^\/read\/saga\/issue-2/);
    // The tracker learns about the replace: the exit target stays put.
    flight.noteRoute("/read/saga/issue-2");
    expect(flight.readerExitOrigin()).toEqual({
      pathname: "/series/saga",
      steps: 1,
    });

    fireEvent.keyDown(window, { key: "P", shiftKey: true });
    await waitFor(() => expect(router.replace).toHaveBeenCalledTimes(2));
    expect(router.replace.mock.calls[1]![0]).toMatch(/^\/read\/saga\/issue-0/);
    expect(router.push).not.toHaveBeenCalled();
  });
});
