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
