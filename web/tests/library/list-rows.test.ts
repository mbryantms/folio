/**
 * The reading-lists table's pure helpers: a folder's roll-up of its
 * lists' counts, the "Most read" comparator, the captions, and the
 * per-browser density store's fallbacks when storage is missing or
 * throws.
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SavedViewView } from "@/lib/api/types";
import {
  compareByRead,
  inLibraryLabel,
  readCaption,
  readFraction,
  readLabel,
  rollupStats,
  type RowStats,
} from "@/lib/cbl/list-rows";
import {
  DEFAULT_DENSITY,
  readDensity,
  writeDensity,
} from "@/lib/saved-views/board-density";

function s(over: Partial<RowStats>): RowStats {
  return {
    total: 0,
    matched: 0,
    read_count: 0,
    ambiguous: 0,
    missing: 0,
    manual: 0,
    ...over,
  };
}

function v(id: string, name = id): SavedViewView {
  return { id, name } as unknown as SavedViewView;
}

describe("rollupStats", () => {
  it("sums every list and skips the ones without stats", () => {
    const r = rollupStats([
      s({ total: 12, matched: 10, read_count: 2, missing: 2, ambiguous: 0 }),
      null,
      s({ total: 5, matched: 5, read_count: 3, ambiguous: 1, manual: 1 }),
      undefined,
    ]);
    expect(r).toEqual(
      s({
        total: 17,
        matched: 15,
        read_count: 5,
        missing: 2,
        ambiguous: 1,
        manual: 1,
      }),
    );
    expect(readLabel(r)).toBe("5 of 15");
    expect(inLibraryLabel(r)).toBe("15 of 17");
    expect(readCaption(r)).toBe("5 of 15 read");
  });

  it("never counts more read than matched", () => {
    expect(rollupStats([s({ matched: 2, read_count: 9 })]).read_count).toBe(2);
    expect(readLabel(s({ matched: 2, read_count: 9 }))).toBe("2 of 2");
  });
});

describe("compareByRead", () => {
  it("puts the most-read first, unknown and empty lists last, ties by name", () => {
    const stats: Record<string, RowStats | null> = {
      half: s({ matched: 10, read_count: 5 }),
      done: s({ matched: 4, read_count: 4 }),
      none: s({ matched: 8, read_count: 0 }),
      empty: s({ matched: 0, total: 6, missing: 6 }),
      unknown: null,
      alsoHalf: s({ matched: 2, read_count: 1 }),
    };
    const views = Object.keys(stats).map((id) => v(id));
    const sorted = views.slice().sort(compareByRead((x) => stats[x.id]));
    expect(sorted.map((x) => x.id)).toEqual([
      "done",
      "alsoHalf",
      "half",
      "none",
      "empty",
      "unknown",
    ]);
    expect(readFraction(stats.empty)).toBeNull();
  });
});

describe("density store", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("defaults to compact without storage and keeps a choice when storage throws", () => {
    const store = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    vi.stubGlobal("window", { localStorage: store });
    expect(readDensity("k")).toBe(DEFAULT_DENSITY);
    writeDensity("k", "comfortable");
    expect(readDensity("k")).toBe("comfortable");
  });

  it("reads back what was written, ignoring junk", () => {
    const data = new Map<string, string>();
    vi.stubGlobal("window", {
      localStorage: {
        getItem: (k: string) => data.get(k) ?? null,
        setItem: (k: string, val: string) => void data.set(k, val),
      },
    });
    writeDensity("k2", "comfortable");
    expect(readDensity("k2")).toBe("comfortable");
    data.set("k2", "huge");
    expect(readDensity("k2")).toBe("compact");
  });
});
