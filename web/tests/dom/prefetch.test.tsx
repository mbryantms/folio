// @vitest-environment jsdom
import { act, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useReaderPrefetch } from "@/lib/reader/use-prefetch";
afterEach(() => vi.unstubAllGlobals());
it("warms nearest forward pages before older pages and stops on unmount", async () => {
  const urls: string[] = [];
  const waiting: (() => void)[] = [];
  class ImageStub {
    naturalWidth = 100;
    naturalHeight = 100;
    fetchPriority = "";
    set src(value: string) {
      if (value) urls.push(value);
    }
    decode() {
      return new Promise<void>((resolve) => waiting.push(resolve));
    }
  }
  vi.stubGlobal("Image", ImageStub);
  const { unmount } = renderHook(() =>
    useReaderPrefetch({
      issueId: "a",
      currentPage: 4,
      totalPages: 20,
      currentGroupIdx: 0,
      groups: [],
      viewMode: "single",
    }),
  );
  expect(urls.slice(0, 4)).toEqual(
    [5, 6, 7, 3].map((n) => `/issues/a/pages/${n}`),
  );
  unmount();
  await act(async () => waiting.forEach((resolve) => resolve()));
  expect(urls).toHaveLength(4);
});

it("turning a page keeps the loads still inside the new window (T3.6)", async () => {
  const started: string[] = [];
  const cancelled: string[] = [];
  const waiting = new Map<string, () => void>();
  class ImageStub {
    naturalWidth = 100;
    naturalHeight = 100;
    fetchPriority = "";
    private url = "";
    set src(value: string) {
      if (value) {
        this.url = value;
        started.push(value);
      } else if (this.url) {
        cancelled.push(this.url);
      }
    }
    decode() {
      return new Promise<void>((resolve) => waiting.set(this.url, resolve));
    }
  }
  vi.stubGlobal("Image", ImageStub);
  const { rerender, unmount } = renderHook(
    ({ page }: { page: number }) =>
      useReaderPrefetch({
        issueId: "a",
        currentPage: page,
        totalPages: 20,
        currentGroupIdx: 0,
        groups: [],
        viewMode: "single",
      }),
    { initialProps: { page: 3 } },
  );
  // Window around 3: 4, 5, 6 ahead, 2, 1 behind; 4 in flight at once.
  expect(started).toEqual([4, 5, 6, 2].map((n) => `/issues/a/pages/${n}`));
  rerender({ page: 4 });
  // Page 5, 6 (and 2) are still wanted — not cancelled, not restarted.
  expect(cancelled).toEqual([]);
  expect(started.filter((u) => u.endsWith("/pages/5"))).toHaveLength(1);
  // Page 1 is now outside the window and never starts; page 7 is new.
  await act(async () => {
    waiting.get("/issues/a/pages/4")?.();
  });
  expect(started).toContain("/issues/a/pages/7");
  expect(started).not.toContain("/issues/a/pages/1");
  // Jumping far away cancels what is no longer wanted.
  rerender({ page: 15 });
  expect(cancelled).toEqual(
    expect.arrayContaining(["/issues/a/pages/5", "/issues/a/pages/6"]),
  );
  unmount();
});
