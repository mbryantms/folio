import { describe, expect, it } from "vitest";

import {
  isDirty,
  selectionDiff,
  toggleSelection,
} from "@/components/admin/users/library-access-logic";

describe("library-access matrix logic", () => {
  it("toggleSelection adds a missing id", () => {
    const next = toggleSelection(new Set(["a"]), "b");
    expect(Array.from(next).sort()).toEqual(["a", "b"]);
  });

  it("toggleSelection removes an existing id", () => {
    const next = toggleSelection(new Set(["a", "b"]), "a");
    expect(Array.from(next)).toEqual(["b"]);
  });

  it("isDirty returns false when selection equals original", () => {
    expect(isDirty(new Set(["a", "b"]), new Set(["a", "b"]))).toBe(false);
    expect(isDirty(new Set(), new Set())).toBe(false);
  });

  it("isDirty detects size mismatches and id mismatches", () => {
    expect(isDirty(new Set(["a"]), new Set(["a", "b"]))).toBe(true);
    expect(isDirty(new Set(["a", "b"]), new Set(["a", "c"]))).toBe(true);
  });

  it("selectionDiff produces stable add/remove buckets", () => {
    const original = new Set(["a", "b", "c"]);
    const selected = new Set(["b", "c", "d"]);
    const diff = selectionDiff(original, selected);
    expect(diff.added).toEqual(["d"]);
    expect(diff.removed).toEqual(["a"]);
  });

  it("selectionDiff returns empty buckets for an unchanged selection", () => {
    const same = new Set(["x", "y"]);
    const diff = selectionDiff(same, new Set(same));
    expect(diff.added).toEqual([]);
    expect(diff.removed).toEqual([]);
  });
});
