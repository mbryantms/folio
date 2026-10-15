// @vitest-environment jsdom
/**
 * `store.init` with the account's per-series overrides (post-audit T4.4):
 * the server row beats a stale local value, a local-only override is
 * adopted (pushed) once, a failed fetch (`null`) leaves the local cache in
 * charge, and per-series setters push through the transport.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { readerPrefGet, readerPrefSet } from "@/lib/reader/prefs-storage";
import {
  flushSeriesPrefs,
  setSeriesPrefTransport,
} from "@/lib/reader/series-prefs-sync";
import { useReaderStore } from "@/lib/reader/store";

const S = "series-1";
const transport = vi.fn().mockResolvedValue(null);

function init(
  serverSeriesPrefs?: Parameters<
    ReturnType<typeof useReaderStore.getState>["init"]
  >[0]["serverSeriesPrefs"],
) {
  useReaderStore.getState().init({
    issueId: "issue-1",
    seriesId: S,
    totalPages: 10,
    initialPage: 0,
    initialDirection: "ltr",
    initialViewMode: "single",
    initialFitMode: "width",
    initialCoverSolo: true,
    serverSeriesPrefs,
  });
}

beforeEach(() => {
  localStorage.clear();
  transport.mockClear();
  setSeriesPrefTransport(transport);
});
afterEach(() => {
  flushSeriesPrefs();
  setSeriesPrefTransport(null);
});

describe("reader store — account per-series overrides", () => {
  it("server values beat a stale local cache and refresh it", () => {
    readerPrefSet("fitMode", S, "height");
    readerPrefSet("viewMode", S, "single");
    init({
      fit_mode: "contain",
      view_mode: "double",
      reading_direction: "rtl",
    });
    const s = useReaderStore.getState();
    expect(s.fitMode).toBe("contain");
    expect(s.viewMode).toBe("double");
    expect(s.direction).toBe("rtl");
    expect(readerPrefGet("fitMode", S)).toBe("contain");
    flushSeriesPrefs();
    expect(transport).not.toHaveBeenCalled();
  });

  it("adopts a local-only override once and keeps using it", () => {
    readerPrefSet("direction", S, "rtl");
    init({});
    expect(useReaderStore.getState().direction).toBe("rtl");
    flushSeriesPrefs();
    expect(transport).toHaveBeenCalledWith(S, { reading_direction: "rtl" });
  });

  it("keeps the local cache in charge when the fetch failed", () => {
    readerPrefSet("fitMode", S, "height");
    init(null);
    expect(useReaderStore.getState().fitMode).toBe("height");
    flushSeriesPrefs();
    expect(transport).not.toHaveBeenCalled();
  });

  it("pushes per-series setter writes but not global ones", () => {
    init({});
    const s = useReaderStore.getState();
    s.setFitMode("original");
    s.setCoverSolo(false);
    s.setMarkersHidden(true);
    flushSeriesPrefs();
    expect(transport).toHaveBeenCalledTimes(1);
    expect(transport).toHaveBeenCalledWith(S, {
      fit_mode: "original",
      cover_solo: false,
    });
    expect(readerPrefGet("fitMode", S)).toBe("original");
  });
});
