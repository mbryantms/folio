/**
 * Multi-select M1: pure helpers from `useSelection`. The React hook
 * itself uses `useState`/`useRef` which aren't exercisable in this
 * vitest config (no DOM env), so the meaningful logic — range
 * computation, toggle semantics, anchor handling — was extracted
 * into `computeToggle` / `computeRangeAdd` / `buildIdIndex` and
 * tested directly.
 */
import { describe, expect, it } from "vitest";

import {
  buildIdIndex,
  computeRangeAdd,
  computeToggle,
} from "@/lib/selection/use-selection";

type Item = { id: string };
const items: Item[] = ["a", "b", "c", "d", "e"].map((id) => ({ id }));
const idx = buildIdIndex(items);

describe("computeToggle", () => {
  it("adds an id when not present", () => {
    const next = computeToggle(new Set(), "c", items, idx, null);
    expect([...next]).toEqual(["c"]);
  });

  it("removes an id when present (plain toggle)", () => {
    const next = computeToggle(new Set(["c"]), "c", items, idx, null);
    expect([...next]).toEqual([]);
  });

  it("preserves other selections when toggling", () => {
    const next = computeToggle(new Set(["a", "c"]), "e", items, idx, null);
    expect(new Set(next)).toEqual(new Set(["a", "c", "e"]));
  });

  it("range-selects between anchor and target on shift-click", () => {
    const next = computeToggle(new Set(), "d", items, idx, "b", {
      shiftKey: true,
    });
    expect(new Set(next)).toEqual(new Set(["b", "c", "d"]));
  });

  it("range-selects in reverse order (anchor after target) too", () => {
    const next = computeToggle(new Set(), "b", items, idx, "d", {
      shiftKey: true,
    });
    expect(new Set(next)).toEqual(new Set(["b", "c", "d"]));
  });

  it("merges range select into an existing selection (additive)", () => {
    const next = computeToggle(new Set(["a"]), "c", items, idx, "b", {
      shiftKey: true,
    });
    expect(new Set(next)).toEqual(new Set(["a", "b", "c"]));
  });

  it("falls back to plain toggle when anchor is null", () => {
    const next = computeToggle(new Set(["a"]), "c", items, idx, null, {
      shiftKey: true,
    });
    expect(new Set(next)).toEqual(new Set(["a", "c"]));
  });

  it("falls back to plain toggle when shift-clicking the anchor itself", () => {
    const next = computeToggle(new Set(["a"]), "a", items, idx, "a", {
      shiftKey: true,
    });
    expect(new Set(next)).toEqual(new Set()); // toggled off
  });

  it("falls back to plain toggle when anchor isn't in items[]", () => {
    const next = computeToggle(new Set(), "c", items, idx, "unknown", {
      shiftKey: true,
    });
    expect(new Set(next)).toEqual(new Set(["c"]));
  });
});

describe("computeRangeAdd", () => {
  it("adds every item between two ids inclusive", () => {
    const next = computeRangeAdd(new Set(), "a", "c", items, idx);
    expect(new Set(next)).toEqual(new Set(["a", "b", "c"]));
  });

  it("handles reverse order (toId before fromId)", () => {
    const next = computeRangeAdd(new Set(), "d", "b", items, idx);
    expect(new Set(next)).toEqual(new Set(["b", "c", "d"]));
  });

  it("is a no-op when either id is missing", () => {
    const prev = new Set(["a"]);
    const next = computeRangeAdd(prev, "a", "missing", items, idx);
    expect(new Set(next)).toEqual(new Set(["a"]));
  });

  it("merges with existing selection", () => {
    const next = computeRangeAdd(new Set(["e"]), "a", "b", items, idx);
    expect(new Set(next)).toEqual(new Set(["a", "b", "e"]));
  });
});

describe("buildIdIndex", () => {
  it("maps each id to its position", () => {
    expect(idx.get("a")).toBe(0);
    expect(idx.get("c")).toBe(2);
    expect(idx.get("e")).toBe(4);
    expect(idx.get("missing")).toBeUndefined();
  });
});
