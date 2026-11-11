// @vitest-environment jsdom
/**
 * The drag-to-turn engine is not in the reader's first-load JS
 * (`lib/reader/use-drag-turn.ts`). Through the whole `Reader`: while its
 * chunk is still loading, and when it fails to load (offline with a cold
 * cache), keys and tap zones turn pages exactly as before and the reader
 * does not fall into its error boundary.
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
const { loader } = vi.hoisted(() => ({ loader: vi.fn() }));
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
vi.mock("@/lib/reader/drag-turn-loader", () => ({
  loadDragTurnEngine: loader,
}));

import { Reader } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader";
import { useReaderStore } from "@/lib/reader/store";

function renderReader() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <Reader
        issueId="issue-1"
        seriesId="s1"
        cblSavedViewId={null}
        exitUrl="/series/saga/issues/issue-1"
        totalPages={5}
        initialPage={0}
        pages={[]}
        pageUrlVersion={null}
        manga={null}
        userDefaultDirection={null}
        libraryDefaultDirection={null}
        seriesReadingDirection={null}
        userDefaultFitMode={null}
        userDefaultViewMode="single"
        userDefaultPageStrip={false}
        userDefaultPageAnimation="slide"
        userDefaultCoverSolo={true}
        userKeybinds={{}}
        activityTrackingEnabled={false}
        readingMinActiveMs={30_000}
        readingMinPages={3}
        readingIdleMs={180_000}
        nextUpOverride={null}
        prevUpOverride={null}
      />
    </QueryClientProvider>,
  );
}

async function turnWithKeysAndZones() {
  await waitFor(() => expect(loader).toHaveBeenCalled(), { timeout: 3000 });
  expect(useReaderStore.getState().currentPage).toBe(0);
  fireEvent.keyDown(window, { key: "ArrowRight" });
  await waitFor(() => expect(useReaderStore.getState().currentPage).toBe(1));
  fireEvent.click(
    screen.getAllByRole("button", { name: "Right zone", hidden: true })[0]!,
  );
  await waitFor(() => expect(useReaderStore.getState().currentPage).toBe(2));
  fireEvent.keyDown(window, { key: "ArrowLeft" });
  await waitFor(() => expect(useReaderStore.getState().currentPage).toBe(1));
}

beforeEach(() => {
  HTMLImageElement.prototype.decode = () => Promise.resolve();
  localStorage.setItem("reader:firstRunSeen:v1", "1");
  loader.mockReset();
  vi.stubGlobal(
    "fetch",
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
});

describe("reader with drag-to-turn deferred", () => {
  it("turns pages by key and tap zone while the engine is still loading", async () => {
    loader.mockReturnValue(new Promise(() => {}));
    renderReader();
    await turnWithKeysAndZones();
    expect(screen.queryByText(/something went wrong/i)).toBeNull();
  });

  it("turns pages by key and tap zone when the engine fails to load", async () => {
    loader.mockRejectedValue(
      new TypeError("Failed to fetch dynamically imported module"),
    );
    renderReader();
    await turnWithKeysAndZones();
    expect(screen.queryByText(/something went wrong/i)).toBeNull();
  });
});
