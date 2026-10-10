/**
 * `<CoverageTab>` — the admin metadata Coverage tab renders the snapshot:
 * identifier coverage with gap links, completeness tiers, per-library and
 * per-series gaps, tag sources (flagging ids left unread), tool state and
 * fill costs. Static-markup render over a mocked query.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import type { MetadataCoverageResp } from "@/lib/api/types";

let state: {
  data: MetadataCoverageResp | undefined;
  isLoading: boolean;
  isFetching: boolean;
  refetch: () => void;
} = { data: undefined, isLoading: false, isFetching: false, refetch: () => {} };

vi.mock("@/lib/api/queries", () => ({
  useAdminMetadataCoverage: () => state,
}));

import { CoverageTab } from "@/components/admin/metadata/CoverageTab";

function snapshot(): MetadataCoverageResp {
  return {
    computed_at: "2026-10-10T04:50:00Z",
    cached: true,
    series_total: 4280,
    issues_total: 44201,
    libraries_total: 24,
    identifiers: [
      {
        source: "comicvine",
        label: "ComicVine",
        series_with_id: 4279,
        issues_with_id: 41614,
      },
      {
        source: "metron",
        label: "Metron",
        series_with_id: 43,
        issues_with_id: 1635,
      },
      { source: "gcd", label: "GCD", series_with_id: 32, issues_with_id: 1527 },
    ],
    all_sources_series: 27,
    all_sources_issues: 1505,
    tiers: {
      complete: 40190,
      partial: 1209,
      needs_metadata: 2573,
      accepted: 0,
    },
    missing_core: [
      { field: "external_id", issues: 2573 },
      { field: "summary", issues: 2442 },
      { field: "credits", issues: 1594 },
      { field: "cover_date", issues: 1108 },
      { field: "page_count", issues: 0 },
    ],
    recommended: [
      { field: "title", issues: 36436 },
      { field: "teams", issues: 31082 },
      { field: "genres", issues: 8534 },
      { field: "story_arcs", issues: 764 },
      { field: "cover_primary", issues: 40012 },
    ],
    series_fields: { with_summary: 3781, with_series_type: 17, synced: 34 },
    by_library: [
      {
        library_id: "lib-dc",
        name: "DC",
        slug: "dc",
        issues_total: 19542,
        without_comicvine: 2306,
        without_metron: 19000,
        without_gcd: 19100,
      },
      {
        library_id: "lib-marvel",
        name: "Marvel",
        slug: "marvel",
        issues_total: 19829,
        without_comicvine: 220,
        without_metron: 19500,
        without_gcd: 19600,
      },
    ],
    top_series: [
      {
        source: "comicvine",
        series: [
          {
            slug: "detective-comics-1937",
            name: "Detective Comics",
            year: 1937,
            issues_without_id: 502,
          },
        ],
      },
      { source: "metron", series: [] },
      { source: "gcd", series: [] },
    ],
    tag_sources: [
      {
        tool: "comictagger_comicvine",
        issues: 32889,
        with_comicvine: 32873,
        with_metron: 3,
        with_gcd: 0,
      },
      {
        tool: "comictagger_metron",
        issues: 1887,
        with_comicvine: 520,
        with_metron: 0,
        with_gcd: 0,
      },
      {
        tool: "folio",
        issues: 2650,
        with_comicvine: 2412,
        with_metron: 1529,
        with_gcd: 1524,
      },
    ],
    tools: {
      weekly_refresh_enabled: false,
      weekly_refresh_cron: "0 0 3 * * SUN",
      stale_after_days: 180,
      issue_refresh_enabled: false,
      issue_refresh_per_provider_cap: 200,
      auto_apply_libraries: 0,
      libraries_total: 24,
      coverage_analysed_series: 5,
      series_synced: 34,
      issues_synced: 1733,
    },
    providers: [],
    fill_cost: [
      {
        source: "comicvine",
        issues_without_id: 2587,
        series_without_id: 1,
        via_lists_requests: 50,
        via_detail_requests: 2587,
        daily_budget: 4800,
        list_page_size: 100,
      },
      {
        source: "metron",
        issues_without_id: 42566,
        series_without_id: 4237,
        via_lists_requests: 8700,
        via_detail_requests: 42566,
        daily_budget: 5000,
        list_page_size: 100,
      },
      {
        source: "gcd",
        issues_without_id: 42674,
        series_without_id: 4248,
        via_lists_requests: 8500,
        via_detail_requests: 42674,
        daily_budget: 2000,
        list_page_size: 50,
      },
    ],
  };
}

describe("<CoverageTab>", () => {
  it("renders the loading shell", () => {
    state = {
      data: undefined,
      isLoading: true,
      isFetching: true,
      refetch: () => {},
    };
    const html = renderToStaticMarkup(createElement(CoverageTab));
    expect(html).toContain("Loading");
  });

  it("renders identifier coverage with gap links into the library grid", () => {
    state = {
      data: snapshot(),
      isLoading: false,
      isFetching: false,
      refetch: () => {},
    };
    const html = renderToStaticMarkup(createElement(CoverageTab));
    expect(html).toContain("Identifier coverage");
    expect(html).toContain("4,279 / 4,280");
    expect(html).toContain("41,614 / 44,201");
    // The Metron series gap links to the grid narrowed to Metron.
    expect(html).toContain(
      "/?provider_match=unmatched&amp;provider_source=metron",
    );
    expect(html).toContain("4,237 unmatched");
    expect(html).toContain("42,566 without id");
    expect(html).toContain("All three ids");
  });

  it("renders the completeness tiers, missing core fields and the worklist link", () => {
    state = {
      data: snapshot(),
      isLoading: false,
      isFetching: false,
      refetch: () => {},
    };
    const html = renderToStaticMarkup(createElement(CoverageTab));
    expect(html).toContain("Issue completeness");
    expect(html).toContain("40,190");
    expect(html).toContain("Needs metadata");
    expect(html).toContain("2,573");
    expect(html).toContain("among the 3,782 incomplete");
    expect(html).toContain("Provider id");
    expect(html).toContain("Story arc");
    expect(html).toContain("/?metadata_completeness=needs_metadata");
  });

  it("renders per-library and per-series gaps with links", () => {
    state = {
      data: snapshot(),
      isLoading: false,
      isFetching: false,
      refetch: () => {},
    };
    const html = renderToStaticMarkup(createElement(CoverageTab));
    expect(html).toContain(
      "/?library=lib-dc&amp;provider_match=unmatched&amp;provider_source=comicvine",
    );
    expect(html).toContain("2,306");
    expect(html).toContain("/series/detective-comics-1937?tab=metadata");
    expect(html).toContain("Detective Comics");
  });

  it("flags a tagging tool whose own ids were never read", () => {
    state = {
      data: snapshot(),
      isLoading: false,
      isFetching: false,
      refetch: () => {},
    };
    const html = renderToStaticMarkup(createElement(CoverageTab));
    expect(html).toContain("ComicTagger · Metron");
    expect(html).toContain("ids in tags, not read");
    // ComicTagger · ComicVine captured its ids: no flag on that row.
    const cvRow =
      html
        .split('data-testid="tag-source-comictagger_comicvine"')[1]
        ?.split("</tr>")[0] ?? "";
    expect(cvRow).not.toContain("ids in tags, not read");
  });

  it("renders tool state and fill costs without any fill button", () => {
    state = {
      data: snapshot(),
      isLoading: false,
      isFetching: false,
      refetch: () => {},
    };
    const html = renderToStaticMarkup(createElement(CoverageTab));
    expect(html).toContain("Weekly refresh");
    expect(html).toContain("Off");
    expect(html).toContain("0 of 24 libraries");
    expect(html).toContain("5 of 4,280 series");
    expect(html).toContain("≈ 8,700 requests");
    expect(html).toContain("about 2 days");
    expect(html).toContain("42,674 archive rewrites");
    expect(html).not.toMatch(/Fill all|Start backfill/i);
  });
});
