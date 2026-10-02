// @vitest-environment jsdom
/**
 * WP-7.7 cover sizing: the effective column width of the issue grid's
 * `auto-fill minmax(size, 1fr)` layout, and `useCardSize` instances
 * sharing a storage key staying in sync on the page (custom event) and
 * across browser tabs (`storage` event).
 */
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import {
  CARD_SIZE_EVENT,
  parseStoredCardSize,
  useCardSize,
} from "@/components/library/use-card-size";
import {
  computeColumnsPerRow,
  effectiveColumnWidth,
} from "@/lib/library/grid-window";
import { SERIES_CARD_SIZE } from "@/lib/library/series-card-size";

const KEY = SERIES_CARD_SIZE.storageKey;

describe("effectiveColumnWidth", () => {
  it("matches the CSS grid: (W - gap*(cols-1)) / cols", () => {
    // 1000px, 160px min, 16px gap → floor(1016/176) = 5 columns.
    expect(computeColumnsPerRow(1000, 160)).toBe(5);
    expect(effectiveColumnWidth(1000, 160)).toBeCloseTo((1000 - 64) / 5);
    // 200px min → floor(1016/216) = 4 → (1000-48)/4 = 238.
    expect(effectiveColumnWidth(1000, 200)).toBe(238);
    // Exact fit: 3*200 + 2*16 = 632 → 3 columns of exactly 200.
    expect(effectiveColumnWidth(632, 200)).toBe(200);
    // One px short of fitting 3 → 2 wider columns.
    expect(effectiveColumnWidth(631, 200)).toBe((631 - 16) / 2);
  });

  it("never drops below one column and falls back before measuring", () => {
    expect(effectiveColumnWidth(100, 160)).toBe(100);
    expect(effectiveColumnWidth(0, 160)).toBe(160);
    expect(effectiveColumnWidth(-5, 240)).toBe(240);
  });

  it("is never narrower than the slider value when it fits", () => {
    for (let w = 300; w <= 1600; w += 37) {
      for (const size of [120, 160, 200, 240, 280]) {
        expect(effectiveColumnWidth(w, size)).toBeGreaterThanOrEqual(size);
      }
    }
  });
});

describe("useCardSize sync", () => {
  beforeEach(() => localStorage.clear());

  const opts = SERIES_CARD_SIZE;

  it("rehydrates from storage", () => {
    localStorage.setItem(KEY, "220");
    const { result } = renderHook(() => useCardSize(opts));
    expect(result.current[0]).toBe(220);
  });

  it("syncs every instance sharing the key on the page", () => {
    const a = renderHook(() => useCardSize(opts));
    const b = renderHook(() => useCardSize(opts));
    const other = renderHook(() =>
      useCardSize({ ...opts, storageKey: "folio.other.cardSize" }),
    );
    act(() => a.result.current[1](240));
    expect(a.result.current[0]).toBe(240);
    expect(b.result.current[0]).toBe(240);
    expect(other.result.current[0]).toBe(opts.defaultSize);
    expect(localStorage.getItem(KEY)).toBe("240");
  });

  it("clamps broadcast values to the instance's bounds", () => {
    const a = renderHook(() => useCardSize(opts));
    act(() => {
      window.dispatchEvent(
        new CustomEvent(CARD_SIZE_EVENT, { detail: { key: KEY, value: 999 } }),
      );
    });
    expect(a.result.current[0]).toBe(opts.max);
  });

  it("follows other browser tabs through the storage event", () => {
    const a = renderHook(() => useCardSize(opts));
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: KEY, newValue: "180" }),
      );
    });
    expect(a.result.current[0]).toBe(180);
    // Another tab clearing the key resets to the default.
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: KEY, newValue: null }),
      );
    });
    expect(a.result.current[0]).toBe(opts.defaultSize);
    // Unrelated keys are ignored.
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: "nope", newValue: "200" }),
      );
    });
    expect(a.result.current[0]).toBe(opts.defaultSize);
  });

  it("parses stored values defensively", () => {
    expect(parseStoredCardSize(null, 120, 280)).toBeNull();
    expect(parseStoredCardSize("", 120, 280)).toBeNull();
    expect(parseStoredCardSize("abc", 120, 280)).toBeNull();
    expect(parseStoredCardSize("100", 120, 280)).toBe(120);
    expect(parseStoredCardSize("200", 120, 280)).toBe(200);
  });
});
