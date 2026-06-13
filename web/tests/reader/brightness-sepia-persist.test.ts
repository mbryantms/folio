/**
 * 2.5 / E4 — brightness & sepia persistence. The vision-comfort sliders
 * used to reset on every full reload; they now persist globally so the
 * setting survives reloads and issue switches. Node-env harness has no
 * `window`, so we stand up a Map-backed localStorage and exercise the
 * store setters → loader round-trip, including the clamp-on-rehydrate
 * guard that keeps a hand-tampered value inside the slider range.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  loadBrightness,
  loadSepia,
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
    },
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("reader brightness/sepia persistence", () => {
  it("returns null before anything is stored", () => {
    expect(loadBrightness()).toBeNull();
    expect(loadSepia()).toBeNull();
  });

  it("persists setter values under the global key and reloads them", () => {
    useReaderStore.getState().setBrightness(1.2);
    useReaderStore.getState().setSepia(0.4);

    expect(store.get("reader:brightness:_default")).toBe("1.2");
    expect(store.get("reader:sepia:_default")).toBe("0.4");
    expect(loadBrightness()).toBe(1.2);
    expect(loadSepia()).toBe(0.4);
  });

  it("clamps setter input to the slider range", () => {
    useReaderStore.getState().setBrightness(99);
    useReaderStore.getState().setSepia(-1);
    expect(useReaderStore.getState().brightness).toBe(1.5);
    expect(useReaderStore.getState().sepia).toBe(0);
    expect(loadBrightness()).toBe(1.5);
    expect(loadSepia()).toBe(0);
  });

  it("clamps a tampered persisted value on rehydrate", () => {
    store.set("reader:brightness:_default", "5");
    store.set("reader:sepia:_default", "9");
    expect(loadBrightness()).toBe(1.5);
    expect(loadSepia()).toBe(1);
  });

  it("ignores a non-numeric persisted value", () => {
    store.set("reader:brightness:_default", "bright");
    expect(loadBrightness()).toBeNull();
  });
});
