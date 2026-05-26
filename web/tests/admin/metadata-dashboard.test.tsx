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
        review_queue_count: number;
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

vi.mock("@/lib/api/queries", () => ({
  useAdminMetadataDashboard: () => queryState,
}));

import { DashboardTab } from "@/components/admin/metadata/DashboardTab";

describe("<DashboardTab>", () => {
  it("renders the loading shell", () => {
    queryState = { data: undefined, isLoading: true };
    const html = renderToStaticMarkup(createElement(DashboardTab));
    expect(html).toContain("Loading");
  });

  it("renders matched percent + provider quota for live data", () => {
    queryState = {
      isLoading: false,
      data: {
        series_total: 100,
        series_matched: 60,
        series_unmatched: 40,
        review_queue_count: 12,
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
    expect(html).toContain("Review queue");
    expect(html).toContain("ComicVine");
    expect(html).toContain("Metron");
    expect(html).toContain("ENABLED");
    expect(html).toContain("NOT CONFIGURED");
    expect(html).toContain("180 /hr");
    expect(html).toContain("resets in");
  });
});
