/**
 * M2 — series-override helper. Pure logic over a `KVStore` interface, so we
 * exercise it against an in-memory map without bringing jsdom into vitest.
 */
import { beforeEach, describe, expect, it } from "vitest";
import {
  SERIES_OVERRIDE_PREFIX,
  clearSeriesOverrides,
  hasSeriesOverrides,
  seriesOverrideKeys,
} from "@/lib/reader/series-overrides";

class FakeStore {
  private map = new Map<string, string>();

  get length(): number {
    return this.map.size;
  }
  key(i: number): string | null {
    return [...this.map.keys()][i] ?? null;
  }
  setItem(k: string, v: string) {
    this.map.set(k, v);
  }
  removeItem(k: string) {
    this.map.delete(k);
  }
  has(k: string) {
    return this.map.has(k);
  }
}

describe("series-overrides", () => {
  let store: FakeStore;

  beforeEach(() => {
    store = new FakeStore();
  });

  it("returns no keys for an unknown series", () => {
    store.setItem(`${SERIES_OVERRIDE_PREFIX}fitMode:other`, "width");
    expect(seriesOverrideKeys(store, "series-1")).toEqual([]);
    expect(hasSeriesOverrides(store, "series-1")).toBe(false);
  });

  it("returns every reader: key that ends with the series id", () => {
    store.setItem(`${SERIES_OVERRIDE_PREFIX}fitMode:s1`, "width");
    store.setItem(`${SERIES_OVERRIDE_PREFIX}viewMode:s1`, "double");
    store.setItem(`${SERIES_OVERRIDE_PREFIX}direction:s1`, "rtl");
    store.setItem(`${SERIES_OVERRIDE_PREFIX}fitMode:s2`, "height");
    store.setItem("unrelated:s1", "noise");
    const keys = seriesOverrideKeys(store, "s1").sort();
    expect(keys).toEqual([
      `${SERIES_OVERRIDE_PREFIX}direction:s1`,
      `${SERIES_OVERRIDE_PREFIX}fitMode:s1`,
      `${SERIES_OVERRIDE_PREFIX}viewMode:s1`,
    ]);
    expect(hasSeriesOverrides(store, "s1")).toBe(true);
  });

  it("ignores keys without the reader: prefix even if they end with the id", () => {
    store.setItem("not-reader:fitMode:s1", "width");
    expect(seriesOverrideKeys(store, "s1")).toEqual([]);
  });

  it("does not match keys that merely contain the id mid-string", () => {
    store.setItem(`${SERIES_OVERRIDE_PREFIX}fitMode:s1-extra`, "width");
    expect(seriesOverrideKeys(store, "s1")).toEqual([]);
  });

  it("clearSeriesOverrides removes only the matching keys and returns them", () => {
    store.setItem(`${SERIES_OVERRIDE_PREFIX}fitMode:s1`, "width");
    store.setItem(`${SERIES_OVERRIDE_PREFIX}viewMode:s1`, "double");
    store.setItem(`${SERIES_OVERRIDE_PREFIX}fitMode:s2`, "height");
    const cleared = clearSeriesOverrides(store, "s1");
    expect(cleared.sort()).toEqual([
      `${SERIES_OVERRIDE_PREFIX}fitMode:s1`,
      `${SERIES_OVERRIDE_PREFIX}viewMode:s1`,
    ]);
    expect(store.has(`${SERIES_OVERRIDE_PREFIX}fitMode:s1`)).toBe(false);
    expect(store.has(`${SERIES_OVERRIDE_PREFIX}viewMode:s1`)).toBe(false);
    expect(store.has(`${SERIES_OVERRIDE_PREFIX}fitMode:s2`)).toBe(true);
    expect(hasSeriesOverrides(store, "s1")).toBe(false);
  });

  it("treats empty seriesId as no-overrides", () => {
    store.setItem(`${SERIES_OVERRIDE_PREFIX}fitMode:`, "width");
    expect(seriesOverrideKeys(store, "")).toEqual([]);
    expect(clearSeriesOverrides(store, "")).toEqual([]);
  });
});
