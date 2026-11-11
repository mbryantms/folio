// @vitest-environment jsdom
/**
 * WP-7.7 cover sizing: the real column width of a fill-the-row cover
 * grid, and `useCardSize` (the bookmarks' own px slider) instances
 * sharing a storage key staying in sync on the page (custom event) and
 * across browser tabs (`storage` event). The app-wide cover size has its
 * own tests (cover-size-store.test.tsx).
 */
import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import {
  CARD_SIZE_EVENT,
  parseStoredCardSize,
  useCardSize,
} from "@/components/library/use-card-size";
import {
  BOOKMARKS_CARD_SIZE_KEY,
  COVER_SIZE_KEY,
} from "@/lib/library/cover-size";
import {
  computeColumnsPerRow,
  effectiveColumnWidth,
  GRID_GAP_PX,
} from "@/lib/library/grid-window";

const KEY = BOOKMARKS_CARD_SIZE_KEY;
const opts = { storageKey: KEY, min: 100, max: 220, defaultSize: 140 };

describe("effectiveColumnWidth", () => {
  it("shares the whole row between the columns that fit", () => {
    // The row never ends in a blank strip: four columns of at least
    // 200px fit 1000px, and each takes a quarter of what the gaps leave.
    expect(computeColumnsPerRow(1000, 200)).toBe(4);
    expect(effectiveColumnWidth(1000, 200)).toBe(238);
    expect(effectiveColumnWidth(1000, 160)).toBeCloseTo((1000 - 64) / 5);
    // Exactly two columns of 200 + one gap: nothing left to share.
    expect(effectiveColumnWidth(416, 200)).toBe(200);
  });

  it("falls back to the target before measuring", () => {
    expect(effectiveColumnWidth(0, 160)).toBe(160);
    expect(effectiveColumnWidth(-5, 240)).toBe(240);
  });

  it("fills the row at every width, never narrower than the target", () => {
    for (let w = 300; w <= 1600; w += 37) {
      for (const size of [120, 160, 200, 240, 280]) {
        const cols = computeColumnsPerRow(w, size);
        const width = effectiveColumnWidth(w, size);
        // Columns + gaps add up to the row exactly.
        expect(cols * width + (cols - 1) * GRID_GAP_PX).toBeCloseTo(w);
        if (w >= 2 * size + GRID_GAP_PX) {
          expect(width).toBeGreaterThanOrEqual(size);
        } else {
          // Narrower than two cards at the target: the two-column floor
          // wins (phone-layout.md) and the row is split in half.
          expect(cols).toBe(2);
          expect(width).toBeCloseTo((w - GRID_GAP_PX) / 2);
        }
      }
    }
  });
});

describe("useCardSize sync", () => {
  beforeEach(() => localStorage.clear());

  it("rehydrates from storage", () => {
    localStorage.setItem(KEY, "180");
    const { result } = renderHook(() => useCardSize(opts));
    expect(result.current[0]).toBe(180);
  });

  it("syncs every instance sharing the key on the page", () => {
    const a = renderHook(() => useCardSize(opts));
    const b = renderHook(() => useCardSize(opts));
    const other = renderHook(() =>
      useCardSize({ ...opts, storageKey: "folio.other.cardSize" }),
    );
    act(() => a.result.current[1](200));
    expect(a.result.current[0]).toBe(200);
    expect(b.result.current[0]).toBe(200);
    expect(other.result.current[0]).toBe(opts.defaultSize);
    expect(localStorage.getItem(KEY)).toBe("200");
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

  it("is not moved by the app-wide cover size", () => {
    const a = renderHook(() => useCardSize(opts));
    act(() => {
      localStorage.setItem(COVER_SIZE_KEY, "200");
      window.dispatchEvent(
        new StorageEvent("storage", { key: COVER_SIZE_KEY, newValue: "200" }),
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
