// @vitest-environment jsdom
/**
 * F7 — the global display prefs follow the account. `store.init` with the
 * account's `/auth/me` slices: a set slice beats the local cache and
 * refreshes it; an unset slice keeps the local value and is adopted on
 * the first change (never on read); an unknown account (`null`) leaves
 * the local cache in charge; every global setter pushes one coalesced
 * PATCH through the transport.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  flushAccountPrefs,
  setAccountPrefTransport,
} from "@/lib/reader/account-prefs-sync";
import { DEFAULT_FILTERS } from "@/lib/reader/filters";
import {
  DEFAULT_SCOPE,
  readerPrefGet,
  readerPrefSet,
} from "@/lib/reader/prefs-storage";
import { useReaderStore } from "@/lib/reader/store";

const transport = vi.fn().mockResolvedValue(null);

type Account = NonNullable<
  Parameters<
    ReturnType<typeof useReaderStore.getState>["init"]
  >[0]["serverAccountPrefs"]
>;

const UNSET: Account = {
  reader_brightness: null,
  reader_sepia: null,
  reader_grayscale: null,
  reader_invert: null,
  reader_warmth: null,
  reader_contrast: null,
  reader_zoom_persist: null,
};

function init(serverAccountPrefs?: Account | null) {
  useReaderStore.getState().init({
    issueId: "issue-1",
    seriesId: "series-1",
    totalPages: 10,
    initialPage: 0,
    initialDirection: "ltr",
    initialViewMode: "single",
    initialFitMode: "width",
    initialCoverSolo: true,
    serverSeriesPrefs: null,
    serverAccountPrefs,
  });
}

beforeEach(() => {
  localStorage.clear();
  transport.mockClear();
  useReaderStore.setState({ ...DEFAULT_FILTERS, zoomPersist: false });
  setAccountPrefTransport(transport);
});
afterEach(() => {
  flushAccountPrefs();
  setAccountPrefTransport(null);
});

describe("reader store — account display prefs", () => {
  it("account values beat a stale local cache and refresh it, without a write", () => {
    readerPrefSet("brightness", DEFAULT_SCOPE, "0.7");
    readerPrefSet("invert", DEFAULT_SCOPE, "false");
    init({
      ...UNSET,
      reader_brightness: 1.2,
      reader_sepia: 0.35,
      reader_grayscale: 40,
      reader_invert: true,
      reader_warmth: -20,
      reader_contrast: 110,
      reader_zoom_persist: true,
    });
    const s = useReaderStore.getState();
    expect(s.brightness).toBe(1.2);
    expect(s.sepia).toBe(0.35);
    expect(s.grayscale).toBe(40);
    expect(s.invert).toBe(true);
    expect(s.warmth).toBe(-20);
    expect(s.contrast).toBe(110);
    expect(s.zoomPersist).toBe(true);
    expect(readerPrefGet("brightness", DEFAULT_SCOPE)).toBe("1.2");
    expect(readerPrefGet("invert", DEFAULT_SCOPE)).toBe("true");
    flushAccountPrefs();
    expect(transport).not.toHaveBeenCalled();
  });

  it("keeps a local value the account has never set and adopts it on the first change only", () => {
    readerPrefSet("brightness", DEFAULT_SCOPE, "1.3");
    readerPrefSet("zoomPersist", DEFAULT_SCOPE, "true");
    init(UNSET);
    expect(useReaderStore.getState().brightness).toBe(1.3);
    expect(useReaderStore.getState().zoomPersist).toBe(true);
    // Reading is not a write.
    flushAccountPrefs();
    expect(transport).not.toHaveBeenCalled();

    useReaderStore.getState().setContrast(120);
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledTimes(1);
    expect(transport).toHaveBeenCalledWith({
      reader_brightness: 1.3,
      reader_zoom_persist: true,
      reader_contrast: 120,
    });

    // The adoption rode along once; later writes carry only their slice.
    useReaderStore.getState().setWarmth(10);
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledTimes(2);
    expect(transport).toHaveBeenLastCalledWith({ reader_warmth: 10 });
  });

  it("leaves the local cache in charge and adopts nothing when the account is unknown", () => {
    readerPrefSet("sepia", DEFAULT_SCOPE, "0.5");
    init(null);
    expect(useReaderStore.getState().sepia).toBe(0.5);
    useReaderStore.getState().setGrayscale(30);
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledTimes(1);
    expect(transport).toHaveBeenCalledWith({ reader_grayscale: 30 });
  });

  it("coalesces a burst of changes into one PATCH with the latest values", () => {
    init({ ...UNSET, reader_brightness: 1 });
    const s = useReaderStore.getState();
    s.setBrightness(1.1);
    s.setBrightness(1.2);
    s.setInvert(true);
    s.setZoomPersist(true);
    s.setMarkersHidden(true); // not an account slice
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledTimes(1);
    expect(transport).toHaveBeenCalledWith({
      reader_brightness: 1.2,
      reader_invert: true,
      reader_zoom_persist: true,
    });
    expect(readerPrefGet("brightness", DEFAULT_SCOPE)).toBe("1.2");
  });

  it("resetFilters writes every filter default in one PATCH", () => {
    init({ ...UNSET, reader_brightness: 1.4, reader_invert: true });
    useReaderStore.getState().resetFilters();
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledTimes(1);
    expect(transport).toHaveBeenCalledWith({
      reader_brightness: 1,
      reader_sepia: 0,
      reader_grayscale: 0,
      reader_invert: false,
      reader_warmth: 0,
      reader_contrast: 100,
    });
  });

  it("adopts a tampered local value clamped to the slider range, never raw", () => {
    readerPrefSet("brightness", DEFAULT_SCOPE, "5");
    readerPrefSet("warmth", DEFAULT_SCOPE, "-999");
    readerPrefSet("invert", DEFAULT_SCOPE, "yes");
    init(UNSET);
    useReaderStore.getState().setContrast(110);
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledWith({
      reader_brightness: 1.5,
      reader_warmth: -50,
      reader_contrast: 110,
    });
  });

  it("re-queues a failed PATCH behind the next push", async () => {
    readerPrefSet("sepia", DEFAULT_SCOPE, "0.4");
    init(UNSET);
    transport.mockRejectedValueOnce(new Error("offline"));
    useReaderStore.getState().setContrast(120);
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledWith({
      reader_sepia: 0.4,
      reader_contrast: 120,
    });
    await Promise.resolve();

    useReaderStore.getState().setContrast(130);
    flushAccountPrefs();
    expect(transport).toHaveBeenCalledTimes(2);
    expect(transport).toHaveBeenLastCalledWith({
      reader_sepia: 0.4,
      reader_contrast: 130,
    });
  });
});
