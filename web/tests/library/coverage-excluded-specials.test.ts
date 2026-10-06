/**
 * Coverage analysis card: the "Not analyzed: …" line built from the
 * analysis' `excluded_specials` (annuals / one-shots / specials the
 * scanner tagged, which are not part of the series' run).
 */
import { describe, expect, it } from "vitest";

import { describeExcludedSpecials } from "@/components/library/ProviderCoverageAnalysis";

describe("describeExcludedSpecials", () => {
  it("groups by type with pluralized labels and the first numbers", () => {
    expect(
      describeExcludedSpecials([
        { number: "1", special_type: "Annual" },
        { number: "2", special_type: "Annual" },
        { number: "1", special_type: "Special" },
      ]),
    ).toBe("2 annuals (#1, #2), 1 special (#1)");
  });

  it("caps the listed numbers and tolerates unnumbered files", () => {
    const annuals = ["1", "2", "3", "4", "5"].map((n) => ({
      number: n,
      special_type: "Annual",
    }));
    expect(describeExcludedSpecials(annuals)).toBe(
      "5 annuals (#1, #2, #3, #4, …)",
    );
    expect(
      describeExcludedSpecials([{ number: null, special_type: "OneShot" }]),
    ).toBe("1 one-shot");
    expect(describeExcludedSpecials(undefined)).toBe("");
  });
});
