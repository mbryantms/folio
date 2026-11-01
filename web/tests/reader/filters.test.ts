/**
 * F6 — reader colour filters: each adjustment alone and combined yields
 * the expected composed CSS `filter`, the defaults yield none, the clamps
 * pin every slider range, and the store's `resetFilters` clears all six
 * (persisting the defaults so a reload stays reset).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  DEFAULT_FILTERS,
  clampBrightness,
  clampContrast,
  clampGrayscale,
  clampSepia,
  clampWarmth,
  composeFilter,
  isDefaultFilters,
} from "@/lib/reader/filters";
import {
  loadContrast,
  loadGrayscale,
  loadInvert,
  loadWarmth,
  useReaderStore,
} from "@/lib/reader/store";

describe("composeFilter", () => {
  it("is undefined at the defaults so the style attribute is dropped", () => {
    expect(composeFilter(DEFAULT_FILTERS)).toBeUndefined();
    expect(isDefaultFilters(DEFAULT_FILTERS)).toBe(true);
  });

  it.each([
    [{ brightness: 1.2 }, "brightness(1.2)"],
    [{ sepia: 0.4 }, "sepia(0.4)"],
    [{ grayscale: 50 }, "grayscale(0.5)"],
    [{ invert: true }, "invert(1)"],
    [{ warmth: 20 }, "sepia(0.2)"],
    [{ warmth: -30 }, "invert(1) sepia(0.3) invert(1)"],
    [{ contrast: 125 }, "contrast(1.25)"],
  ] as const)("renders %o alone as %s", (patch, expected) => {
    const f = { ...DEFAULT_FILTERS, ...patch };
    expect(composeFilter(f)).toBe(expected);
    expect(isDefaultFilters(f)).toBe(false);
  });

  it("composes every filter in a fixed order: invert first, warmth last", () => {
    expect(
      composeFilter({
        brightness: 0.8,
        sepia: 0.1,
        grayscale: 100,
        invert: true,
        warmth: 50,
        contrast: 80,
      }),
    ).toBe(
      "invert(1) brightness(0.8) contrast(0.8) grayscale(1) sepia(0.1) sepia(0.5)",
    );
    expect(
      composeFilter({ ...DEFAULT_FILTERS, brightness: 1.1, warmth: -50 }),
    ).toBe("brightness(1.1) invert(1) sepia(0.5) invert(1)");
  });
});

describe("filter clamps", () => {
  it("pin each slider range and round away float noise", () => {
    expect(clampBrightness(0.1)).toBe(0.5);
    expect(clampBrightness(9)).toBe(1.5);
    expect(clampSepia(0.1 + 0.2)).toBe(0.3);
    expect(clampSepia(-1)).toBe(0);
    expect(clampSepia(2)).toBe(1);
    expect(clampGrayscale(-5)).toBe(0);
    expect(clampGrayscale(101)).toBe(100);
    expect(clampGrayscale(33.3)).toBe(33);
    expect(clampWarmth(-80)).toBe(-50);
    expect(clampWarmth(80)).toBe(50);
    expect(clampContrast(0)).toBe(50);
    expect(clampContrast(999)).toBe(150);
  });
});

describe("store filters + reset", () => {
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
    useReaderStore.getState().resetFilters();
    store.clear();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("setters clamp, persist under the global scope and rehydrate", () => {
    const s = useReaderStore.getState();
    s.setGrayscale(150);
    s.setInvert(true);
    s.setWarmth(-70);
    s.setContrast(20);
    expect(useReaderStore.getState()).toMatchObject({
      grayscale: 100,
      invert: true,
      warmth: -50,
      contrast: 50,
    });
    expect(store.get("reader.v1:grayscale:_default")).toBe("100");
    expect(store.get("reader.v1:invert:_default")).toBe("true");
    expect(store.get("reader.v1:warmth:_default")).toBe("-50");
    expect(store.get("reader.v1:contrast:_default")).toBe("50");
    expect(loadGrayscale()).toBe(100);
    expect(loadInvert()).toBe(true);
    expect(loadWarmth()).toBe(-50);
    expect(loadContrast()).toBe(50);
  });

  it("clamps a tampered persisted value and ignores garbage", () => {
    store.set("reader.v1:contrast:_default", "400");
    store.set("reader.v1:warmth:_default", "warm");
    store.set("reader.v1:invert:_default", "yes");
    expect(loadContrast()).toBe(150);
    expect(loadWarmth()).toBeNull();
    expect(loadInvert()).toBeNull();
  });

  it("resetFilters returns all six to the defaults and persists them", () => {
    const s = useReaderStore.getState();
    s.setBrightness(1.3);
    s.setSepia(0.5);
    s.setGrayscale(40);
    s.setInvert(true);
    s.setWarmth(25);
    s.setContrast(120);
    useReaderStore.getState().resetFilters();
    const after = useReaderStore.getState();
    expect(
      composeFilter({
        brightness: after.brightness,
        sepia: after.sepia,
        grayscale: after.grayscale,
        invert: after.invert,
        warmth: after.warmth,
        contrast: after.contrast,
      }),
    ).toBeUndefined();
    expect(store.get("reader.v1:brightness:_default")).toBe("1");
    expect(store.get("reader.v1:invert:_default")).toBe("false");
    expect(store.get("reader.v1:contrast:_default")).toBe("100");
  });
});
