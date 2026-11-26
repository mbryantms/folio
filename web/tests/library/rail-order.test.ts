import { describe, expect, it } from "vitest";

import { moveRail, restoreRail } from "@/lib/pages/rail-order";

describe("moveRail", () => {
  const ids = ["a", "b", "c", "d"];

  it("moves one place up or down", () => {
    expect(moveRail(ids, "c", -1)).toEqual(["a", "c", "b", "d"]);
    expect(moveRail(ids, "b", 1)).toEqual(["a", "c", "b", "d"]);
  });

  it("returns the same array at the ends and for an unknown id", () => {
    expect(moveRail(ids, "a", -1)).toBe(ids);
    expect(moveRail(ids, "d", 1)).toBe(ids);
    expect(moveRail(ids, "zz", 1)).toBe(ids);
  });

  it("clamps a longer jump", () => {
    expect(moveRail(ids, "a", 10)).toEqual(["b", "c", "d", "a"]);
  });
});

describe("restoreRail", () => {
  it("puts a removed rail back where it was", () => {
    expect(restoreRail(["a", "c"], "b", 1)).toEqual(["a", "b", "c"]);
    expect(restoreRail(["a", "b"], "c", 2)).toEqual(["a", "b", "c"]);
  });

  it("clamps an index past the end and never duplicates", () => {
    expect(restoreRail(["a"], "b", 9)).toEqual(["a", "b"]);
    expect(restoreRail(["a", "b"], "b", 0)).toEqual(["b", "a"]);
  });
});
