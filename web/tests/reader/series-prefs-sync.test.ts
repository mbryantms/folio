// @vitest-environment jsdom
/**
 * `series-prefs-sync` (post-audit T4.4): the server row is mirrored over
 * the local cache, local-only overrides are reported for one-time
 * adoption, slice writes map to the wire fields, and pushes within the
 * coalescing window collapse into one PUT per series.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readerPrefGet, readerPrefSet } from "@/lib/reader/prefs-storage";
import {
  PUSH_COALESCE_MS,
  flushSeriesPrefs,
  mirrorServerPrefs,
  patchForSlice,
  pendingAdoption,
  pushSeriesPref,
  setSeriesPrefTransport,
} from "@/lib/reader/series-prefs-sync";

const S = "series-1";

beforeEach(() => {
  localStorage.clear();
  vi.useFakeTimers();
});
afterEach(() => {
  flushSeriesPrefs();
  setSeriesPrefTransport(null);
  vi.useRealTimers();
});

describe("mirrorServerPrefs", () => {
  it("overwrites the local cache with every slice the server has", () => {
    readerPrefSet("fitMode", S, "height");
    readerPrefSet("direction", S, "ltr");
    mirrorServerPrefs(S, {
      fit_mode: "contain",
      view_mode: "double",
      cover_solo: false,
    });
    expect(readerPrefGet("fitMode", S)).toBe("contain");
    expect(readerPrefGet("viewMode", S)).toBe("double");
    expect(readerPrefGet("coverSolo", S)).toBe("false");
    // A slice the server lacks keeps its local value.
    expect(readerPrefGet("direction", S)).toBe("ltr");
  });
});

describe("pendingAdoption", () => {
  it("reports local overrides the server has never seen, once", () => {
    readerPrefSet("direction", S, "rtl");
    readerPrefSet("coverSolo", S, "true");
    readerPrefSet("fitMode", S, "width");
    expect(pendingAdoption(S, { fit_mode: "width" })).toEqual({
      reading_direction: "rtl",
      cover_solo: true,
    });
    expect(
      pendingAdoption(S, {
        fit_mode: "width",
        reading_direction: "rtl",
        cover_solo: true,
      }),
    ).toBeNull();
    expect(pendingAdoption("other", {})).toBeNull();
  });
});

describe("patchForSlice", () => {
  it("maps store slices onto the wire fields", () => {
    expect(patchForSlice("fitMode", "height")).toEqual({ fit_mode: "height" });
    expect(patchForSlice("viewMode", "webtoon")).toEqual({
      view_mode: "webtoon",
    });
    expect(patchForSlice("direction", "rtl")).toEqual({
      reading_direction: "rtl",
    });
    expect(patchForSlice("coverSolo", "false")).toEqual({ cover_solo: false });
  });
});

describe("pushSeriesPref", () => {
  it("coalesces writes within the window into one PUT per series", async () => {
    const transport = vi.fn().mockResolvedValue(null);
    setSeriesPrefTransport(transport);
    pushSeriesPref(S, "fitMode", "height");
    pushSeriesPref(S, "fitMode", "contain");
    pushSeriesPref(S, "direction", "rtl");
    pushSeriesPref("series-2", "viewMode", "double");
    expect(transport).not.toHaveBeenCalled();
    vi.advanceTimersByTime(PUSH_COALESCE_MS + 1);
    expect(transport).toHaveBeenCalledTimes(2);
    expect(transport).toHaveBeenCalledWith(S, {
      fit_mode: "contain",
      reading_direction: "rtl",
    });
    expect(transport).toHaveBeenCalledWith("series-2", { view_mode: "double" });
  });

  it("is a no-op without a transport and swallows transport failures", () => {
    pushSeriesPref(S, "fitMode", "height");
    vi.advanceTimersByTime(PUSH_COALESCE_MS + 1);
    const failing = vi.fn().mockRejectedValue(new Error("offline"));
    setSeriesPrefTransport(failing);
    pushSeriesPref(S, "fitMode", "height");
    expect(() => flushSeriesPrefs()).not.toThrow();
    expect(failing).toHaveBeenCalledTimes(1);
  });
});
