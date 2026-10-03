// @vitest-environment jsdom
/**
 * <ProviderDetectResults> — the per-provider outcome of "Detect from
 * providers": resolution method, gap outcomes, stale mappings, and the
 * confirm flow for a medium-confidence candidate.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ProviderDetectResults } from "@/components/library/ProviderDetectResults";
import type { DetectResp, DetectSourceResult } from "@/lib/api/types";

function result(over: Partial<DetectSourceResult>): DetectSourceResult {
  return {
    source: "gcd",
    source_label: "Grand Comics Database",
    status: "scanned",
    provider_series_id: null,
    provider_series_name: null,
    provider_series_year: null,
    provider_series_url: null,
    resolved_via: null,
    id_recorded: false,
    covered_count: 0,
    matched_local: 0,
    gaps: [],
    gap_details: [],
    created: [],
    stale_ranges: [],
    uncovered_specials: 0,
    candidates: [],
    error: null,
    ...over,
  };
}

const resp: DetectResp = {
  agreement: {
    agree: false,
    summary:
      "Providers split this run differently (Metron #501; GCD #500–501).",
  },
  results: [
    result({
      source: "metron",
      source_label: "Metron",
      status: "needs_confirmation",
      candidates: [
        {
          external_id: "1711",
          name: "Fantastic Four",
          year: 1962,
          publisher: null,
          url: "https://metron.cloud/series/1711/",
          score: 67.5,
          issue_overlap: null,
          reason: "start year differs",
        },
      ],
    }),
    result({
      status: "scanned",
      provider_series_id: "1482",
      provider_series_name: "Fantastic Four",
      provider_series_year: 1961,
      provider_series_url: "https://www.comics.org/series/1482/",
      resolved_via: "search",
      id_recorded: true,
      covered_count: 416,
      matched_local: 4,
      gaps: ["500..501"],
      gap_details: [
        {
          low: "500",
          high: "501",
          issue_count: 2,
          status: "mapped",
          provider_series_id: "9999",
          provider_series_name: "Fantastic Four",
          error: null,
        },
      ],
      uncovered_specials: 1,
      stale_ranges: [
        {
          id: "r1",
          source: "gcd",
          source_label: "Grand Comics Database",
          provider_series_id: "4242",
          provider_series_url: null,
          provider_series_name: "Old Split",
          range_low: "600",
          range_high: "611",
          declared_year: null,
          set_by: "cross_reference",
          first_set_at: "2026-10-01T00:00:00Z",
          last_synced_at: "2026-10-01T00:00:00Z",
        },
      ],
    }),
    result({
      source: "comicvine",
      source_label: "ComicVine",
      status: "not_enumerable",
      provider_series_id: "2045",
      resolved_via: "linked",
    }),
  ],
};

describe("<ProviderDetectResults>", () => {
  it("renders each provider's status, link and gap outcomes", () => {
    render(
      <ProviderDetectResults
        result={resp}
        confirmingId={null}
        onConfirm={() => undefined}
        onRemoveStale={() => undefined}
      />,
    );
    expect(screen.getByText("Needs confirmation")).toBeTruthy();
    expect(screen.getByText("Can't list issues")).toBeTruthy();
    expect(screen.getByText(/via series search/)).toBeTruthy();
    expect(screen.getByText(/saved to external IDs/)).toBeTruthy();
    expect(
      screen.getByText(
        /#500–501 \(2 issues\) → mapped to Fantastic Four #9999/,
      ),
    ).toBeTruthy();
    expect(screen.getByText(/1 annual\/special issue isn't/)).toBeTruthy();
    expect(
      screen.getByText(/Mapping #600–611 → Old Split looks stale/),
    ).toBeTruthy();
    expect(
      screen.getByText(/Providers split this run differently/),
    ).toBeTruthy();
  });

  it("confirms a candidate and removes a stale mapping via callbacks", () => {
    const onConfirm = vi.fn();
    const onRemoveStale = vi.fn();
    render(
      <ProviderDetectResults
        result={resp}
        confirmingId={null}
        onConfirm={onConfirm}
        onRemoveStale={onRemoveStale}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Use this series/ }));
    expect(onConfirm).toHaveBeenCalledWith(
      "metron",
      expect.objectContaining({ external_id: "1711" }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Remove stale Grand Comics Database mapping",
      }),
    );
    expect(onRemoveStale).toHaveBeenCalledWith(
      "Grand Comics Database",
      expect.objectContaining({ id: "r1" }),
    );
  });

  it("disables confirmation while one is in flight", () => {
    render(
      <ProviderDetectResults
        result={resp}
        confirmingId="metron:1711"
        onConfirm={() => undefined}
        onRemoveStale={() => undefined}
      />,
    );
    const btn = screen.getByRole("button", {
      name: /Use this series/,
    }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });
});
