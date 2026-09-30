/**
 * Pure helpers behind the page-text panel and the text-region keyboard
 * proxies (WP-4.8): reading order, nested-box de-duplication, the bounded
 * in-order OCR pool, and the recognizer → `lang` mapping. Also pins the
 * panel's default shortcut against the keybind registry.
 */
import { describe, expect, it } from "vitest";
import type { TextRegionView } from "@/lib/api/types";
import {
  ocrLangTag,
  readingOrderRegions,
  runInOrderPool,
} from "@/lib/reader/page-text";
import {
  KEYBIND_DEFAULTS,
  KEYBIND_LABELS,
  READER_KEYBIND_ACTIONS,
  findConflict,
} from "@/lib/reader/keybinds";

function r(x: number, y: number, w: number, h: number): TextRegionView {
  return { x, y, w, h, confidence: 0.9, class: 0 };
}

describe("readingOrderRegions", () => {
  it("orders rows top-to-bottom, left-to-right in LTR", () => {
    const a = r(60, 10, 20, 10); // top row, right
    const b = r(10, 12, 20, 10); // top row (staggered), left
    const c = r(10, 60, 20, 10); // bottom row
    expect(readingOrderRegions([c, a, b], "ltr")).toEqual([b, a, c]);
  });

  it("orders each row right-to-left in RTL", () => {
    const a = r(60, 10, 20, 10);
    const b = r(10, 12, 20, 10);
    const c = r(10, 60, 20, 10);
    expect(readingOrderRegions([c, a, b], "rtl")).toEqual([a, b, c]);
  });

  it("drops boxes nested inside a larger one so text isn't read twice", () => {
    const block = r(10, 10, 40, 20);
    const line = r(12, 12, 30, 5); // fully inside `block`
    const other = r(70, 10, 20, 10);
    expect(readingOrderRegions([line, block, other], "ltr")).toEqual([
      block,
      other,
    ]);
  });

  it("keeps exactly one of two identical boxes and skips empty ones", () => {
    const a = r(10, 10, 20, 10);
    const dup = r(10, 10, 20, 10);
    const empty = r(50, 50, 0, 10);
    expect(readingOrderRegions([a, dup, empty], "ltr")).toEqual([a]);
  });

  it("keeps partially overlapping neighbours", () => {
    const a = r(10, 10, 20, 10);
    const b = r(25, 10, 20, 10); // 25% overlap with a
    expect(readingOrderRegions([b, a], "ltr")).toEqual([a, b]);
  });
});

describe("runInOrderPool", () => {
  it("starts work in order and never exceeds the concurrency cap", async () => {
    const started: number[] = [];
    let inFlight = 0;
    let peak = 0;
    await runInOrderPool([0, 1, 2, 3, 4], 2, async (item) => {
      started.push(item);
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await new Promise((res) => setTimeout(res, 1));
      inFlight -= 1;
    });
    expect(started).toEqual([0, 1, 2, 3, 4]);
    expect(peak).toBe(2);
  });

  it("stops launching new work once the signal aborts", async () => {
    const ctrl = new AbortController();
    const started: number[] = [];
    await runInOrderPool(
      [0, 1, 2, 3],
      1,
      async (item) => {
        started.push(item);
        if (item === 1) ctrl.abort();
      },
      ctrl.signal,
    );
    expect(started).toEqual([0, 1]);
  });

  it("is a no-op for an empty list", async () => {
    let calls = 0;
    await runInOrderPool([], 2, async () => {
      calls += 1;
    });
    expect(calls).toBe(0);
  });
});

describe("ocrLangTag", () => {
  it("maps recognizers to BCP-47 tags", () => {
    expect(ocrLangTag("manga")).toBe("ja");
    expect(ocrLangTag("western")).toBe("en");
    expect(ocrLangTag(null)).toBeUndefined();
  });
});

describe("togglePageText keybind", () => {
  it("is a reader action bound to a free default key", () => {
    expect(READER_KEYBIND_ACTIONS).toContain("togglePageText");
    expect(KEYBIND_DEFAULTS.togglePageText).toBe("r");
    expect(KEYBIND_LABELS.togglePageText).toBe("Show page text");
    expect(
      findConflict("r", "togglePageText", { ...KEYBIND_DEFAULTS }),
    ).toBeNull();
  });
});
