// @vitest-environment jsdom
/**
 * The first render of a reader (lib/reader/use-seeded-page.ts).
 *
 * Regression: resuming an issue on page 14 mounted the store's stale page
 * first (0, or the last issue's page), which downloaded that page at high
 * priority ahead of the real one and played a page turn from it.
 */
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { useReaderStore } from "@/lib/reader/store";
import { usePageTransition } from "@/lib/reader/use-page-transition";
import { useSeededPage } from "@/lib/reader/use-seeded-page";

const seed = (issueId: string, initialPage: number, totalPages = 24) =>
  useReaderStore.getState().init({
    issueId,
    seriesId: "s1",
    totalPages,
    initialPage,
    initialDirection: "ltr",
    initialViewMode: "single",
    initialFitMode: "width",
    initialPageStripVisible: false,
    initialCoverSolo: true,
    serverSeriesPrefs: null,
    serverAccountPrefs: null,
  });

describe("useSeededPage", () => {
  beforeEach(() => {
    // Left over from the issue read before this one.
    useReaderStore.setState({ currentPage: 12, totalPages: 40 });
  });

  it("shows the page the open lands on before the store is seeded", () => {
    const { result } = renderHook(() =>
      useSeededPage({ issueId: "i1", initialPage: 5, totalPages: 24 }),
    );
    expect(result.current).toEqual({ currentPage: 5, storeReady: false });

    act(() => seed("i1", 5));
    expect(result.current).toEqual({ currentPage: 5, storeReady: true });
  });

  it("follows the store once seeded", () => {
    const { result } = renderHook(() =>
      useSeededPage({ issueId: "i1", initialPage: 5, totalPages: 24 }),
    );
    act(() => seed("i1", 5));
    act(() => useReaderStore.getState().setPage(6));
    expect(result.current.currentPage).toBe(6);
  });

  it("clamps an open past the end, as the seed does", () => {
    const { result } = renderHook(() =>
      useSeededPage({ issueId: "i1", initialPage: 99, totalPages: 24 }),
    );
    expect(result.current.currentPage).toBe(23);
  });

  it("is unready again in the same render an in-place issue change arrives", () => {
    const { result, rerender } = renderHook(
      (p: { issueId: string; initialPage: number }) =>
        useSeededPage({ ...p, totalPages: 24 }),
      { initialProps: { issueId: "i1", initialPage: 5 } },
    );
    act(() => seed("i1", 5));
    act(() => useReaderStore.getState().setPage(20));

    rerender({ issueId: "i2", initialPage: 0 });
    expect(result.current).toEqual({ currentPage: 0, storeReady: false });

    act(() => seed("i2", 0));
    expect(result.current).toEqual({ currentPage: 0, storeReady: true });
  });

  it("opening an issue plays no page turn; turning a page still does", () => {
    const { result } = renderHook(() => {
      const { currentPage } = useSeededPage({
        issueId: "i1",
        initialPage: 5,
        totalPages: 24,
      });
      return usePageTransition({
        currentPage,
        direction: "ltr",
        mode: "slide",
      });
    });
    expect(result.current.prevPage).toBeNull();

    act(() => seed("i1", 5));
    expect(result.current.prevPage).toBeNull();

    act(() => useReaderStore.getState().setPage(6));
    expect(result.current.prevPage).toBe(5);
  });
});
