import { describe, expect, it } from "vitest";

import {
  buildAccessRequest,
  capsDirty,
  setCap,
} from "@/components/admin/users/library-access-logic";
import { AGE_RATING_LADDER } from "@/lib/age-rating";

describe("library-access age-rating caps (WP-2.7)", () => {
  it("ladder matches the ComicInfo vocabulary, youngest first", () => {
    expect(AGE_RATING_LADDER[0]).toBe("Early Childhood");
    expect(AGE_RATING_LADDER.indexOf("Teen")).toBeLessThan(
      AGE_RATING_LADDER.indexOf("Mature 17+"),
    );
    expect(AGE_RATING_LADDER[AGE_RATING_LADDER.length - 1]).toBe("X18+");
    expect(new Set(AGE_RATING_LADDER).size).toBe(AGE_RATING_LADDER.length);
  });

  it("setCap stores a value and clears on empty / null", () => {
    const caps = setCap(new Map(), "a", "Teen");
    expect(caps.get("a")).toBe("Teen");
    expect(setCap(caps, "a", null).has("a")).toBe(false);
    expect(setCap(caps, "a", "").has("a")).toBe(false);
  });

  it("capsDirty ignores caps on unselected libraries", () => {
    const original = new Map([["a", "Teen"]]);
    expect(capsDirty(original, new Map([["a", "Teen"]]), new Set(["a"]))).toBe(
      false,
    );
    expect(capsDirty(original, new Map([["a", "PG"]]), new Set(["a"]))).toBe(
      true,
    );
    expect(capsDirty(original, new Map(), new Set(["a"]))).toBe(true);
    // "b" carries a cap but isn't selected — it will be dropped on save.
    expect(
      capsDirty(
        original,
        new Map([
          ["a", "Teen"],
          ["b", "PG"],
        ]),
        new Set(["a"]),
      ),
    ).toBe(false);
  });

  it("buildAccessRequest only sends caps for selected libraries", () => {
    const req = buildAccessRequest(
      new Set(["a", "b"]),
      new Map([
        ["a", "Teen"],
        ["c", "PG"],
      ]),
    );
    expect(req.library_ids.sort()).toEqual(["a", "b"]);
    expect(req.age_rating_caps).toEqual({ a: "Teen" });
  });
});
