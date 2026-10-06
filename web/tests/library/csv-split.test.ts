/**
 * `lib/metadata/csv`: the web twin of the server's `split_csv`, including
 * the generational-suffix rule ("José Marzán, Jr." is one person).
 */
import { describe, expect, it } from "vitest";

import { primaryCsvEntry, splitCsv } from "@/lib/metadata/csv";

describe("splitCsv", () => {
  it("splits on commas, trims, dedupes case-insensitively", () => {
    expect(splitCsv("Action, Adventure, action ")).toEqual([
      "Action",
      "Adventure",
    ]);
    expect(splitCsv("")).toEqual([]);
    expect(splitCsv(null)).toEqual([]);
  });

  it("uses `;` alone when present so names may contain commas", () => {
    expect(splitCsv("Capes, Inc.; Comet Twins")).toEqual([
      "Capes, Inc.",
      "Comet Twins",
    ]);
  });

  it("re-attaches a generational suffix to the preceding name", () => {
    expect(splitCsv("Andrew Hennessy, Mike Deodato, Jr., J. P. Mayer")).toEqual(
      ["Andrew Hennessy", "Mike Deodato Jr.", "J. P. Mayer"],
    );
    expect(splitCsv("J. Jonah Jameson, Sr")).toEqual(["J. Jonah Jameson Sr."]);
    expect(splitCsv("Timothy Green, II")).toEqual(["Timothy Green II"]);
    expect(splitCsv("Mike Deodato Jr., Mike Deodato, Jr.")).toEqual([
      "Mike Deodato Jr.",
    ]);
    expect(splitCsv("Jr., Alice")).toEqual(["Jr.", "Alice"]);
    expect(splitCsv("Hugo Strange, V")).toEqual(["Hugo Strange", "V"]);
    expect(splitCsv("Capes, Inc.; Jr.")).toEqual(["Capes, Inc.", "Jr."]);
  });
});

describe("primaryCsvEntry", () => {
  it("returns the first split entry, suffix attached", () => {
    expect(primaryCsvEntry("Mike Deodato, Jr., Dan Brown")).toBe(
      "Mike Deodato Jr.",
    );
    expect(primaryCsvEntry("  ")).toBeNull();
    expect(primaryCsvEntry(null)).toBeNull();
  });
});
