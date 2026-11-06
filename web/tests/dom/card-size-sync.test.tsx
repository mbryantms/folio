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
  GRID_GAP_PX,
} from "@/lib/library/grid-window";
import { SERIES_CARD_SIZE } from "@/lib/library/series-card-size";

const KEY = SERIES_CARD_SIZE.storageKey;

describe("effectiveColumnWidth", () => {
  it("is exactly the slider value whenever two cards fit", () => {
    // Regression: the grid used `minmax(<size>px, 1fr)`, so 200px
    // rendered 238px wide at 1000px while the home rail showed 200px.
    expect(computeColumnsPerRow(1000, 200)).toBe(4);
    expect(effectiveColumnWidth(1000, 200)).toBe(200);
    expect(effectiveColumnWidth(1000, 160)).toBe(160);
    expect(effectiveColumnWidth(632, 200)).toBe(200);
    expect(effectiveColumnWidth(631, 200)).toBe(200);
  });

  it("falls back to the slider value before measuring", () => {
    expect(effectiveColumnWidth(0, 160)).toBe(160);
    expect(effectiveColumnWidth(-5, 240)).toBe(240);
  });

  it("matches the slider value, or half the row under the phone floor", () => {
    for (let w = 300; w <= 1600; w += 37) {
      for (const size of [120, 160, 200, 240, 280]) {
        if (w >= 2 * size + GRID_GAP_PX) {
          expect(effectiveColumnWidth(w, size)).toBe(size);
        } else {
          // Narrower than two cards at the slider value: the two-column
          // floor wins (phone-layout.md) and the row is split in half.
          expect(computeColumnsPerRow(w, size)).toBe(2);
          expect(effectiveColumnWidth(w, size)).toBeCloseTo(
            (w - GRID_GAP_PX) / 2,
          );
        }
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
