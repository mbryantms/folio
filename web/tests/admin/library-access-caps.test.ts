import { describe, expect, it } from "vitest";

import {
  type AgeRatingHistogram,
  buildAccessRequest,
  capEffect,
  capsDirty,
  describeCapEffect,
  hideUnratedDirty,
  loadGrants,
  setCap,
  setHideUnrated,
} from "@/components/admin/users/library-access-logic";
import {
  AGE_RATING_GROUPS,
  AGE_RATING_LADDER,
  canonicalCap,
  groupOf,
  ladderRank,
} from "@/lib/age-rating";

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
    expect(req.hide_unrated).toEqual({});
  });

  it("buildAccessRequest sends the hide-unrated set for selected libraries", () => {
    const req = buildAccessRequest(
      new Set(["a", "b"]),
      new Map(),
      new Set(["b", "c"]),
    );
    expect(req.hide_unrated).toEqual({ b: true });
  });

  it("every stored ladder value loads and saves back byte for byte", () => {
    for (const rung of AGE_RATING_LADDER) {
      const loaded = loadGrants([
        { library_id: "a", age_rating_max: rung, hide_unrated: true },
      ]);
      // Through the same loader the form uses: a re-spelled or dropped
      // canonical cap fails here.
      expect(loaded.caps.get("a")).toBe(rung);
      expect(loaded.unenforcedCaps.size).toBe(0);
      const req = buildAccessRequest(
        loaded.selected,
        loaded.caps,
        loaded.hideUnrated,
      );
      expect(req).toEqual({
        library_ids: ["a"],
        age_rating_caps: { a: rung },
        hide_unrated: { a: true },
      });
    }
  });

  it("loads a stored cap the way the server enforces it", () => {
    const loaded = loadGrants([
      { library_id: "lower", age_rating_max: "teen", hide_unrated: false },
      { library_id: "padded", age_rating_max: " Teen\t", hide_unrated: false },
      { library_id: "alias", age_rating_max: "Mature", hide_unrated: false },
      { library_id: "junk", age_rating_max: "PG-13ish", hide_unrated: false },
      { library_id: "none", age_rating_max: null, hide_unrated: false },
    ]);
    // Another spelling of a rung: enforced by the server, loads as the rung.
    expect(loaded.caps.get("lower")).toBe("Teen");
    expect(loaded.caps.get("padded")).toBe("Teen");
    // Not a rung: the server enforces no cap, so the form shows none and
    // says so; saving sends no cap rather than a value the API refuses.
    expect(loaded.caps.has("alias")).toBe(false);
    expect(loaded.caps.has("junk")).toBe(false);
    expect([...loaded.unenforcedCaps]).toEqual([
      ["alias", "Mature"],
      ["junk", "PG-13ish"],
    ]);
    expect(loaded.caps.has("none")).toBe(false);
    const req = buildAccessRequest(
      loaded.selected,
      loaded.caps,
      loaded.hideUnrated,
    );
    expect(req.age_rating_caps).toEqual({ lower: "Teen", padded: "Teen" });
  });

  it("canonicalCap trims only the server's characters", () => {
    expect(canonicalCap("teen")).toBe("Teen");
    expect(canonicalCap(" Teen ")).toBe("Teen");
    expect(canonicalCap("\v\fx18+\r\n")).toBe("X18+");
    // A non-breaking space is not in the server's trim set.
    expect(canonicalCap("\u00a0Teen")).toBeNull();
    expect(canonicalCap("Mature")).toBeNull();
    expect(canonicalCap("Unknown")).toBeNull();
    expect(canonicalCap("")).toBeNull();
    expect(canonicalCap(null)).toBeNull();
  });

  it("hideUnratedDirty only looks at selected libraries", () => {
    const original = new Set(["a"]);
    expect(hideUnratedDirty(original, new Set(["a"]), new Set(["a"]))).toBe(
      false,
    );
    expect(hideUnratedDirty(original, new Set(), new Set(["a"]))).toBe(true);
    expect(
      hideUnratedDirty(original, new Set(["a", "b"]), new Set(["a"])),
    ).toBe(false);
    const on = setHideUnrated(new Set(), "a", true);
    expect([...on]).toEqual(["a"]);
    expect(setHideUnrated(on, "a", false).size).toBe(0);
  });
});

describe("age-rating groups (U-09)", () => {
  it("cover every rung once, in ladder order", () => {
    const flat = AGE_RATING_GROUPS.flatMap((g) => [...g.ratings]);
    expect(flat).toEqual([...AGE_RATING_LADDER]);
    for (const g of AGE_RATING_GROUPS) {
      expect(g.ratings.length).toBeGreaterThan(0);
      expect(g.description.length).toBeGreaterThan(0);
    }
  });

  it("place each rung under a plain heading", () => {
    expect(AGE_RATING_GROUPS.map((g) => [g.heading, [...g.ratings]])).toEqual([
      ["Young children", ["Early Childhood"]],
      ["All ages", ["Everyone", "G"]],
      ["Older children", ["Everyone 10+", "PG", "Kids to Adults"]],
      ["Teens", ["Teen"]],
      ["Older teens", ["MA15+"]],
      ["Mature", ["Mature 17+", "M"]],
      ["Adults only", ["R18+", "Adults Only 18+", "X18+"]],
    ]);
    expect(groupOf("MA15+")?.heading).toBe("Older teens");
    expect(groupOf("MA15+")?.description).toMatch(/restricted/);
    expect(groupOf(null)).toBeNull();
    expect(groupOf("Mature")).toBeNull(); // an alias, never a cap
    expect(ladderRank("Teen")).toBe(6);
    expect(ladderRank("bogus")).toBeNull();
  });
});

describe("capEffect (U-09)", () => {
  // 1 Everyone, 2 Teen, 1 MA15+, 3 Mature 17+, 1 X18+, 4 unrated = 12.
  const counts: Record<string, number> = {
    Everyone: 1,
    Teen: 2,
    "MA15+": 1,
    "Mature 17+": 3,
    "X18+": 1,
  };
  const histogram: AgeRatingHistogram = {
    rungs: AGE_RATING_LADDER.map((rating) => ({
      rating,
      series_count: counts[rating] ?? 0,
    })),
    unrated_count: 4,
    series_count: 12,
  };

  it("no cap, switch off: hides nothing", () => {
    const e = capEffect(histogram, null, false);
    expect(e).toMatchObject({ aboveCap: 0, hidden: 0, unrated: 4, total: 12 });
    expect(describeCapEffect(e)).toBe(
      "Hides none of 12 series; 4 unrated still shown.",
    );
  });

  it("cap only: hides the rungs above it, unrated stay shown", () => {
    const e = capEffect(histogram, "Teen", false);
    expect(e.aboveCap).toBe(5);
    expect(e.hidden).toBe(5);
    expect(describeCapEffect(e)).toBe(
      "Hides 5 of 12 series; 4 unrated still shown.",
    );
    expect(capEffect(histogram, "X18+", false).hidden).toBe(0);
    expect(capEffect(histogram, "Early Childhood", false).hidden).toBe(8);
  });

  it("hide unrated only: hides just the unrated series", () => {
    const e = capEffect(histogram, null, true);
    expect(e.hidden).toBe(4);
    expect(describeCapEffect(e)).toBe(
      "Hides 4 of 12 series; 4 unrated hidden.",
    );
  });

  it("both: adds the two", () => {
    expect(capEffect(histogram, "Mature 17+", true).hidden).toBe(1 + 4);
  });

  it("reads the cap like the server: other spellings rank, non-rungs don't", () => {
    // 1 MA15+ + 3 Mature 17+ + 1 X18+ above Teen.
    expect(capEffect(histogram, "teen", false).hidden).toBe(5);
    expect(capEffect(histogram, " Teen ", false).hidden).toBe(5);
    // An alias is not a cap the server enforces.
    expect(capEffect(histogram, "Mature", false).hidden).toBe(0);
    expect(capEffect(histogram, "garbage", false).hidden).toBe(0);
    expect(capEffect(histogram, "garbage", true).hidden).toBe(4);
  });

  it("leaves out the unrated clause when there are none", () => {
    const e = capEffect(
      { ...histogram, unrated_count: 0, series_count: 8 },
      "Teen",
      true,
    );
    expect(describeCapEffect(e)).toBe("Hides 5 of 8 series.");
  });
});
