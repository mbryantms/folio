/**
 * Series page › Metadata tab: the summary strip, the fetch actions (moved
 * here from the ··· menu), the issue audit table with its filters and
 * bulk actions, files and provenance. Static-markup render over mocked
 * hooks.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import type {
  SeriesMetadataIssuesResp,
  SeriesMetadataOverview,
} from "@/lib/api/types";

const overview: SeriesMetadataOverview = {
  series_id: "s1",
  series_ids: [
    {
      source: "comicvine",
      source_label: "ComicVine",
      external_id: "3824",
      external_url: "https://comicvine.gamespot.com/wonder-woman/4050-3824/",
      set_by: "comicinfo",
      first_set_at: "2026-10-06T00:00:00Z",
      last_synced_at: "2026-10-06T00:00:00Z",
    },
  ],
  issue_ids: { total: 240, comicvine: 73, metron: 2, gcd: 2, all_sources: 2 },
  tiers: { complete: 45, partial: 28, needs_metadata: 167, accepted: 0 },
  sync: {
    series_last_sync_at: "2026-10-10T12:44:20Z",
    metadata_sync_paused: false,
    issues_synced: 3,
    sidecars_rewritten: 240,
    last_rewrite_at: "2026-10-10T16:33:35Z",
  },
  files: {
    archives: 240,
    by_extension: [{ extension: "cbz", count: 240 }],
    with_comicinfo: 240,
    with_metroninfo: 240,
    series_json_present: true,
    cvinfo_present: true,
    backups: 244,
    folder_path: "/library/media/comics/DC Comics/Wonder Woman (1987)",
  },
  last_run: {
    id: "r1",
    scope: "issue",
    status: "completed",
    started_at: "2026-10-10T16:31:08Z",
    items_total: 1,
    items_matched_high: 1,
    items_applied: 0,
  },
  runs_total: 401,
  series_provenance: [
    {
      field: "description",
      set_by: "series_json",
      set_at: "2026-10-06T00:00:00Z",
      source_external_id: null,
    },
    {
      field: "series_type",
      set_by: "metron",
      set_at: "2026-10-10T12:44:00Z",
      source_external_id: "1306",
    },
  ],
  issue_provenance: { file: 240, provider: 3, user: 0 },
};

const issues: SeriesMetadataIssuesResp = {
  total: 4,
  series_total: 240,
  items: [
    {
      id: "a1",
      slug: "annual-1",
      label: "Annual 1",
      number_raw: "1",
      special_type: "Annual",
      title: null,
      sort_number: 1,
      cover_date: "1988-11",
      tier: "complete",
      accepted: false,
      missing_core: [],
      ids: { comicvine: "30585", metron: "13096", gcd: "45539" },
      tagged_by: "folio",
      last_run: {
        id: "r2",
        status: "completed",
        started_at: "2026-10-10T16:16:00Z",
        matched_high: 2,
        applied: 1,
      },
    },
    {
      id: "sf1",
      slug: "secret-files-1",
      label: "Special 1",
      number_raw: "1",
      special_type: "Special",
      title: "Vulnerability",
      sort_number: 1,
      cover_date: "1998-03",
      tier: "partial",
      accepted: false,
      missing_core: ["summary"],
      ids: { comicvine: "44763", metron: null, gcd: null },
      tagged_by: "untagged",
      last_run: {
        id: "r3",
        status: "completed",
        started_at: "2026-10-10T16:31:00Z",
        matched_high: 1,
        applied: 0,
      },
    },
    {
      id: "i176",
      slug: "176",
      label: "#176",
      number_raw: "176",
      special_type: null,
      title: null,
      sort_number: 176,
      cover_date: null,
      tier: "needs_metadata",
      accepted: false,
      missing_core: ["external_id", "cover_date", "summary", "credits"],
      ids: { comicvine: null, metron: null, gcd: null },
      tagged_by: "comictagger_metron",
      last_run: null,
    },
    {
      id: "i2",
      slug: "2",
      label: "#2",
      number_raw: "2",
      special_type: null,
      title: null,
      sort_number: 2,
      cover_date: "1987-03",
      tier: "partial",
      accepted: true,
      missing_core: ["summary"],
      ids: { comicvine: "1", metron: null, gcd: null },
      tagged_by: "mylar",
      last_run: null,
    },
  ],
};

vi.mock("@/lib/api/queries", () => ({
  useSeriesMetadataOverview: () => ({ data: overview, isLoading: false }),
  useSeriesMetadataIssues: () => ({ data: issues, isLoading: false }),
}));
const mutate = vi.fn();
vi.mock("@/lib/api/mutations", () => ({
  useAcceptSeriesIssues: () => ({ mutate, isPending: false }),
  useCreateSeriesBatch: () => ({ mutate, isPending: false }),
  useCreateSeriesSelectionBatch: () => ({ mutate, isPending: false }),
}));
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));
vi.mock("next/dynamic", () => ({ default: () => () => null }));
vi.mock("next/link", () => ({
  default: ({ href, children }: { href: string; children: React.ReactNode }) =>
    createElement("a", { href }, children),
}));
vi.mock("@/components/library/ExternalIdsCard", () => ({
  ExternalIdsCard: () => createElement("div", null, "external-ids-card"),
}));
vi.mock("@/components/library/SeriesProviderRangesCard", () => ({
  SeriesProviderRangesCard: () =>
    createElement("div", null, "provider-ranges-card"),
}));

import { SeriesMetadataTab } from "@/app/[locale]/(library)/series/[slug]/SeriesMetadataTab";
import {
  SERIES_TABS,
  resolveSeriesTab,
} from "@/app/[locale]/(library)/series/[slug]/SeriesTabs";

function render() {
  return renderToStaticMarkup(
    createElement(SeriesMetadataTab, {
      seriesSlug: "wonder-woman-2006",
      seriesName: "Wonder Woman",
      libraryId: "lib-dc",
    }),
  );
}

describe("<SeriesMetadataTab>", () => {
  it("renders the summary strip: series ids, completeness, issue ids, sync", () => {
    const html = render();
    expect(html).toContain("Series identifiers");
    expect(html).toContain("3824");
    expect(html).toContain("none"); // Metron / GCD series ids
    expect(html).toContain("45 complete · 28 partial · 167 need");
    expect(html).toContain("ComicVine 73 · Metron 2 · GCD 2 of 240");
    expect(html).toContain("Issues ever fetched: 3");
    expect(html).toContain("sidecars rewritten: 240");
  });

  it("hosts the fetch actions and the latest run, with no fill-all button", () => {
    const html = render();
    expect(html).toContain("Refresh this series…");
    expect(html).toContain("Match this series…");
    expect(html).toContain("only missing or partial (195)");
    expect(html).toContain("Fetch metadata · all issues");
    expect(html).toContain("Last run: issue · completed");
    expect(html).toContain("401 runs on this series");
    expect(html).not.toMatch(/Fill all|Start backfill/i);
  });

  it("mounts the identifiers and provider-mapping cards moved from Details", () => {
    const html = render();
    expect(html).toContain("external-ids-card");
    expect(html).toContain("provider-ranges-card");
    expect(html).toContain("Provider series mapping");
  });

  it("renders the audit table with tiers, missing fields, ids, tagger and runs", () => {
    const html = render();
    expect(html).toContain('data-testid="series-metadata-issues"');
    expect(html).toContain("Annual 1");
    expect(html).toContain("Vulnerability");
    expect(html).toContain("Needs metadata");
    expect(html).toContain("ComicTagger · Metron");
    expect(html).toContain("Mylar");
    expect(html).toContain("No tags");
    expect(html).toContain("1 strong, not applied");
    expect(html).toContain("· applied");
    expect(html).toContain("never");
    // An acknowledged issue shows as Accepted, not Partial.
    const acceptedRow =
      html.split('data-testid="issue-row-2"')[1]?.split("</tr>")[0] ?? "";
    expect(acceptedRow).toContain("Accepted");
    // Row links go to the issue page's own Metadata tab.
    expect(html).toContain("/series/wonder-woman-2006/issues/176?tab=metadata");
  });

  it("offers the server-side filters with counts from the overview", () => {
    const html = render();
    expect(html).toContain("No Metron id");
    expect(html).toContain("238"); // 240 - 2
    expect(html).toContain("Annuals &amp; specials");
    expect(html).toContain("4 of 240 shown");
  });

  it("renders files and provenance", () => {
    const html = render();
    expect(html).toContain("Files and sidecars");
    expect(html).toContain("CBZ 240");
    expect(html).toContain("244");
    expect(html).toContain("cvinfo");
    expect(html).toContain("Field provenance");
    expect(html).toContain("series_json");
    expect(html).toContain("240 issues carry file-tier values");
  });
});

describe("series tabs", () => {
  it("knows the metadata tab", () => {
    const available = new Set(SERIES_TABS);
    expect(resolveSeriesTab("metadata", available)).toBe("metadata");
    expect(resolveSeriesTab("nope", available)).toBe("credits");
  });
});
