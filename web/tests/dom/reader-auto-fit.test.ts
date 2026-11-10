// @vitest-environment jsdom
/**
 * The fit a reader opens in when nothing chose one (audit U-05): Screen
 * fit on a window wider than the page at full height, width fit on a
 * phone. Any saved fit (account default, the account's per-series row, the
 * local per-series cache) wins on every viewport, and the automatic choice
 * is never written back.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readerPrefGet, readerPrefSet } from "@/lib/reader/prefs-storage";
import {
  flushSeriesPrefs,
  setSeriesPrefTransport,
} from "@/lib/reader/series-prefs-sync";
import { autoFitMode, useReaderStore, type FitMode } from "@/lib/reader/store";
import type { ViewMode } from "@/lib/reader/detect";
import type { SeriesReaderPrefsView } from "@/lib/api/types";

const S = "series-auto-fit";
const transport = vi.fn().mockResolvedValue(null);

const DESKTOP = { width: 1440, height: 900 };
const PHONE = { width: 390, height: 844 };

function viewport({ width, height }: { width: number; height: number }) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    value: width,
  });
  Object.defineProperty(window, "innerHeight", {
    configurable: true,
    value: height,
  });
}

function init({
  initialFitMode,
  initialViewMode = "single",
  serverSeriesPrefs,
}: {
  initialFitMode?: FitMode | null;
  initialViewMode?: ViewMode;
  serverSeriesPrefs?: SeriesReaderPrefsView | null;
} = {}) {
  useReaderStore.getState().init({
    issueId: "issue-1",
    seriesId: S,
    totalPages: 10,
    initialPage: 0,
    initialDirection: "ltr",
    initialViewMode,
    initialFitMode,
    initialPageAspect: 2 / 3,
    initialCoverSolo: true,
    serverSeriesPrefs,
  });
  return useReaderStore.getState().fitMode;
}

beforeEach(() => {
  localStorage.clear();
  transport.mockClear();
  setSeriesPrefTransport(transport);
});
afterEach(() => {
  flushSeriesPrefs();
  setSeriesPrefTransport(null);
  viewport({ width: 1024, height: 768 });
});

describe("reader auto fit", () => {
  it("no saved fit on a wide window opens in Screen fit", () => {
    viewport(DESKTOP);
    expect(init()).toBe("contain");
  });

  it("no saved fit on a phone keeps width fit", () => {
    viewport(PHONE);
    expect(init()).toBe("width");
  });

  it("webtoon keeps its own default on a wide window", () => {
    viewport(DESKTOP);
    expect(init({ initialViewMode: "webtoon" })).toBe("width");
  });

  it.each([DESKTOP, PHONE])("the account default wins at %o", (vp) => {
    viewport(vp);
    expect(init({ initialFitMode: "height" })).toBe("height");
    expect(init({ initialFitMode: "width" })).toBe("width");
  });

  it.each([DESKTOP, PHONE])("the account's per-series row wins at %o", (vp) => {
    viewport(vp);
    expect(init({ serverSeriesPrefs: { fit_mode: "original" } })).toBe(
      "original",
    );
  });

  it.each([DESKTOP, PHONE])("the local per-series cache wins at %o", (vp) => {
    viewport(vp);
    readerPrefSet("fitMode", S, vp === DESKTOP ? "width" : "contain");
    expect(init()).toBe(vp === DESKTOP ? "width" : "contain");
  });

  it("the automatic choice is not saved", () => {
    viewport(DESKTOP);
    expect(init()).toBe("contain");
    flushSeriesPrefs();
    expect(readerPrefGet("fitMode", S)).toBeNull();
    expect(transport).not.toHaveBeenCalled();
  });
});

describe("autoFitMode", () => {
  it("compares the window to the page at full height", () => {
    const at = (w: number, h: number, viewMode: ViewMode = "single") =>
      autoFitMode({
        viewMode,
        pageAspect: 0.5,
        viewportWidth: w,
        viewportHeight: h,
      });
    expect(at(501, 1000)).toBe("contain");
    expect(at(500, 1000)).toBe("width");
    // Double view: the spread is two pages wide.
    expect(at(900, 1000, "double")).toBe("width");
    expect(at(1100, 1000, "double")).toBe("contain");
  });

  it("assumes a portrait 2:3 page without dimensions", () => {
    expect(
      autoFitMode({
        viewMode: "single",
        pageAspect: null,
        viewportWidth: 700,
        viewportHeight: 1000,
      }),
    ).toBe("contain");
    expect(
      autoFitMode({
        viewMode: "single",
        pageAspect: undefined,
        viewportWidth: 600,
        viewportHeight: 1000,
      }),
    ).toBe("width");
  });
});
