/**
 * `<DashboardTab>` smoke — metadata-providers-1.0 M6.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

let queryState: {
  data:
    | undefined
    | {
        series_total: number;
        series_matched: number;
        series_unmatched: number;
        applies_last_7_days: number;
        providers: Array<{
          id: string;
          label: string;
          enabled: boolean;
          configured: boolean;
          quota: {
            remaining_hour: number | null;
            remaining_day: number | null;
            seconds_until_reset: number | null;
          } | null;
        }>;
      };
  isLoading: boolean;
} = { data: undefined, isLoading: false };

let matchQualityState: {
  data:
    | undefined
    | {
        last_7d: Array<{ kind: string; count: number }>;
        last_28d: Array<{ kind: string; count: number }>;
        total_7d: number;
        total_28d: number;
      };
  isLoading: boolean;
} = { data: undefined, isLoading: false };

vi.mock("@/lib/api/queries", () => ({
  useAdminMetadataDashboard: () => queryState,
  useAdminMetadataMatchQuality: () => matchQualityState,
}));

import { DashboardTab } from "@/components/admin/metadata/DashboardTab";

describe("<DashboardTab>", () => {
  it("renders the loading shell", () => {
    queryState = { data: undefined, isLoading: true };
    matchQualityState = { data: undefined, isLoading: true };
    const html = renderToStaticMarkup(createElement(DashboardTab));
    expect(html).toContain("Loading");
  });

  it("renders the match-quality empty-state copy when no runs have been recorded", () => {
    queryState = {
      isLoading: false,
      data: {
        series_total: 1,
        series_matched: 0,
        series_unmatched: 1,
        applies_last_7_days: 0,
        providers: [],
      },
    };
    matchQualityState = {
      isLoading: false,
      data: { last_7d: [], last_28d: [], total_7d: 0, total_28d: 0 },
    };
    const html = renderToStaticMarkup(createElement(DashboardTab));
    expect(html).toContain("Match quality");
    expect(html).toContain("No search runs in the last 28 days");
  });

  it("renders the match-quality 7d + 28d distributions", () => {
    queryState = {
      isLoading: false,
      data: {
        series_total: 100,
        series_matched: 80,
        series_unmatched: 20,
        applies_last_7_days: 12,
        providers: [],
      },
    };
    matchQualityState = {
      isLoading: false,
      data: {
        last_7d: [
          { kind: "multi_good", count: 6 },
          { kind: "single_bad_cover", count: 3 },
        ],
        last_28d: [
          { kind: "multi_good", count: 20 },
          { kind: "single_bad_cover", count: 10 },
          { kind: "no_match", count: 5 },
        ],
        total_7d: 9,
        total_28d: 35,
      },
    };
    const html = renderToStaticMarkup(createElement(DashboardTab));
    expect(html).toContain("Match quality");
    expect(html).toContain("Last 7 days");
    expect(html).toContain("Last 28 days");
    expect(html).toContain("Multiple strong matches");
    expect(html).toContain("One weak match");
    expect(html).toContain("No matches");
    // 7d totals header
    expect(html).toContain("9 total");
    expect(html).toContain("35 total");
  });

  it("renders matched percent + provider quota for live data", () => {
    matchQualityState = {
      isLoading: false,
      data: { last_7d: [], last_28d: [], total_7d: 0, total_28d: 0 },
    };
    queryState = {
      isLoading: false,
      data: {
        series_total: 100,
        series_matched: 60,
        series_unmatched: 40,
        applies_last_7_days: 8,
        providers: [
          {
            id: "comicvine",
            label: "ComicVine",
            enabled: true,
            configured: true,
            quota: {
              remaining_hour: 180,
              remaining_day: null,
              seconds_until_reset: 1800,
            },
          },
          {
            id: "metron",
            label: "Metron",
            enabled: false,
            configured: false,
            quota: null,
          },
        ],
      },
    };
    const html = renderToStaticMarkup(createElement(DashboardTab));
    expect(html).toContain("60");
    expect(html).toContain("60%");
    expect(html).toContain("40");
    expect(html).toContain("ComicVine");
    expect(html).toContain("Metron");
    expect(html).toContain("ENABLED");
    expect(html).toContain("NOT CONFIGURED");
    expect(html).toContain("180 /hr");
    expect(html).toContain("resets in");
  });
});
