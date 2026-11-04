import { describe, expect, it } from "vitest";

import { highlightSegments } from "@/lib/search/highlight";

const marked = (text: string, q: string) =>
  highlightSegments(text, q)
    .map((s) => (s.match ? `[${s.text}]` : s.text))
    .join("");

describe("highlightSegments", () => {
  it("marks a case-insensitive substring and keeps original casing", () => {
    expect(marked("The Amazing Spider-Man", "amaz")).toBe(
      "The [Amaz]ing Spider-Man",
    );
  });

  it("marks every occurrence", () => {
    expect(marked("Saga of Saga", "saga")).toBe("[Saga] of [Saga]");
  });

  it("marks each query term separately (spaces vs hyphens)", () => {
    expect(marked("Spider-Man", "spider man")).toBe("[Spider]-[Man]");
  });

  it("merges overlapping and touching ranges", () => {
    expect(marked("Batman", "bat atm")).toBe("[Batm]an");
    expect(marked("Batman", "bat man")).toBe("[Batman]");
  });

  it("ignores 1-char terms unless they are the whole query", () => {
    expect(marked("X-Men", "x men")).toBe("X-[Men]");
    expect(marked("X-Men", "x")).toBe("[X]-Men");
  });

  it("returns the plain text for no match or an empty query", () => {
    expect(highlightSegments("Saga", "zzz")).toEqual([
      { text: "Saga", match: false },
    ]);
    expect(highlightSegments("Saga", "   ")).toEqual([
      { text: "Saga", match: false },
    ]);
    expect(highlightSegments("", "saga")).toEqual([]);
  });

  it("treats regex metacharacters literally", () => {
    expect(marked("What If...? (1977)", "(1977)")).toBe("What If...? [(1977)]");
    expect(marked("Issue #1.5", ".5")).toBe("Issue #1[.5]");
  });

  it("skips highlighting when lower-casing changes the length", () => {
    // "İ" lower-cases to two code units; indices would drift.
    expect(highlightSegments("İstanbul", "stan")).toEqual([
      { text: "İstanbul", match: false },
    ]);
  });
});
