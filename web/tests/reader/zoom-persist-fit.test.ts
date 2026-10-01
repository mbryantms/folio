/**
 * WP-4.2 store slices: the "keep zoom between pages" preference (global,
 * persisted) and the `contain` ("fit screen") fit mode joining the `f`
 * cycle. Same Map-backed localStorage harness as
 * brightness-sepia-persist.test.ts.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  FIT_MODES,
  loadFitMode,
  loadZoomPersist,
  useReaderStore,
} from "@/lib/reader/store";

let store: Map<string, string>;

beforeEach(() => {
  store = new Map();
  vi.stubGlobal("window", {
    localStorage: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
      get length() {
        return store.size;
      },
      key: (i: number) => [...store.keys()][i] ?? null,
    },
  });
  useReaderStore.setState({
    seriesId: "ser-1",
    fitMode: "width",
    zoomPersist: false,
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("zoomPersist preference", () => {
  it("defaults off", () => {
    expect(loadZoomPersist()).toBe(false);
  });

  it("persists globally (not per series) and survives init", () => {
    useReaderStore.getState().setZoomPersist(true);
    expect(useReaderStore.getState().zoomPersist).toBe(true);
    expect(store.get("reader.v1:zoomPersist:_default")).toBe("true");
    expect(loadZoomPersist()).toBe(true);

    // A fresh issue in another series re-hydrates it.
    useReaderStore.setState({ zoomPersist: false });
    useReaderStore.getState().init({
      issueId: "iss-2",
      seriesId: "ser-2",
      totalPages: 10,
      initialPage: 0,
      initialDirection: "ltr",
      initialViewMode: "single",
    });
    expect(useReaderStore.getState().zoomPersist).toBe(true);
  });

  it("turning it off persists false", () => {
    useReaderStore.getState().setZoomPersist(true);
    useReaderStore.getState().setZoomPersist(false);
    expect(loadZoomPersist()).toBe(false);
  });
});

describe("contain fit mode", () => {
  it("is appended to the f-cycle after original, then wraps to width", () => {
    expect(FIT_MODES).toEqual(["width", "height", "original", "contain"]);
    useReaderStore.setState({ fitMode: "original" });
    useReaderStore.getState().cycleFitMode();
    expect(useReaderStore.getState().fitMode).toBe("contain");
    useReaderStore.getState().cycleFitMode();
    expect(useReaderStore.getState().fitMode).toBe("width");
  });

  it("round-trips through per-series storage", () => {
    useReaderStore.getState().setFitMode("contain");
    expect(loadFitMode("ser-1")).toBe("contain");
  });

  it("is honoured as the user default on init", () => {
    useReaderStore.getState().init({
      issueId: "iss-3",
      seriesId: "ser-3",
      totalPages: 5,
      initialPage: 0,
      initialDirection: "ltr",
      initialViewMode: "single",
      initialFitMode: "contain",
    });
    expect(useReaderStore.getState().fitMode).toBe("contain");
  });
});
