/**
 * Provider-complete search: the per-provider status line the Review queue
 * and the match dialog show next to a result (`lib/metadata/provider-status`).
 */
import { describe, expect, it } from "vitest";

import type { ProviderStatus } from "@/lib/api/types";
import {
  matchedProviders,
  providerCoverage,
  providerCoverageSummary,
  providerStatusLine,
} from "@/lib/metadata/provider-status";

const answered = (source: string, candidates: number): ProviderStatus => ({
  source,
  state: "answered",
  candidates,
});

describe("provider status helpers", () => {
  it("treats an empty list as unknown (pre-bookkeeping runs)", () => {
    expect(providerCoverage([])).toBe("unknown");
    expect(providerCoverageSummary([])).toBeNull();
    expect(providerStatusLine([])).toBe("");
  });

  it("is complete only when every provider answered, matched or not", () => {
    const all = [answered("comicvine", 1), answered("metron", 0)];
    expect(providerCoverage(all)).toBe("complete");
    expect(matchedProviders(all)).toEqual(["comicvine"]);
    expect(providerStatusLine(all)).toBe("ComicVine ✓1 · Metron —");
    expect(providerCoverageSummary(all)).toEqual({
      text: "1 of 2 providers matched",
      partial: false,
    });
  });

  it("flags owed and failed providers as a partial match", () => {
    const list: ProviderStatus[] = [
      answered("comicvine", 2),
      { source: "metron", state: "quota", candidates: 0, retry_after_secs: 90 },
      { source: "gcd", state: "failed", candidates: 0, error: "500" },
    ];
    expect(providerCoverage(list)).toBe("partial");
    expect(providerStatusLine(list)).toBe(
      "ComicVine ✓2 · Metron awaiting quota · GCD failed",
    );
    expect(providerCoverageSummary(list)).toEqual({
      text: "1 of 3 providers matched · Metron awaiting quota, GCD failed",
      partial: true,
    });
  });

  it("says when everything matched", () => {
    const list = [answered("comicvine", 1), answered("metron", 1)];
    expect(providerCoverageSummary(list)).toEqual({
      text: "All 2 providers matched",
      partial: false,
    });
  });
});
