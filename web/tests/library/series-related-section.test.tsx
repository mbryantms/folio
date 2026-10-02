/**
 * <SeriesRelatedSection> — WP-7.1. Static-markup smoke over the mocked
 * relationships query: renders the reading-order chain in position order
 * with this series highlighted, groups direct relationships by kind, gates
 * the admin affordances, and stays invisible when empty for non-admins.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import type { SeriesRelationshipsResp, SeriesView } from "@/lib/api/types";

let role: "admin" | "user" = "user";
let data: SeriesRelationshipsResp | undefined;

vi.mock("@/lib/api/queries", () => ({
  useMe: () => ({ data: { role } }),
  useSeriesRelationships: () => ({ data, isLoading: false }),
  useSeriesListInfinite: () => ({ data: undefined, isLoading: false }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useCreateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
}));

import {
  SeriesRelatedSection,
  chainCaption,
  groupRelationships,
} from "@/components/library/SeriesRelatedSection";

function series(id: string, name: string, year: number): SeriesView {
  return {
    id,
    slug: name.toLowerCase().replace(/\s+/g, "-"),
    name,
    year,
    library_id: "lib",
    status: "continuing",
    language_code: "en",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    cover_url: `/issues/${id}/pages/0/thumb`,
  } as SeriesView;
}

const vol1 = series("s1", "Saga Vol 1", 2012);
const vol2 = series("s2", "Saga Vol 2", 2014);
const vol3 = series("s3", "Saga Vol 3", 2016);
const omnibus = series("s4", "Saga Omnibus", 2020);

const full: SeriesRelationshipsResp = {
  series_id: "s2",
  relationships: [
    {
      id: "r1",
      kind: "sequel_of",
      kind_label: "Sequel of",
      source: "manual",
      confidence: null,
      created_at: "2026-01-01T00:00:00Z",
      series: vol1,
    },
    {
      id: "r2",
      kind: "collected_in",
      kind_label: "Collected in",
      source: "suggested",
      confidence: 0.9,
      created_at: "2026-01-01T00:00:00Z",
      series: omnibus,
    },
    {
      id: "r3",
      kind: "prequel_of",
      kind_label: "Prequel of",
      source: "manual",
      confidence: null,
      created_at: "2026-01-01T00:00:00Z",
      series: vol3,
    },
  ],
  chain: [
    { position: -1, series: vol1 },
    { position: 0, series: vol2 },
    { position: 1, series: vol3 },
  ],
};

function render() {
  return renderToStaticMarkup(
    createElement(SeriesRelatedSection, {
      seriesSlug: "saga-vol-2",
      seriesId: "s2",
    }),
  );
}

describe("<SeriesRelatedSection>", () => {
  it("renders nothing for a non-admin when there are no relationships", () => {
    role = "user";
    data = { series_id: "s2", relationships: [], chain: [] };
    expect(render()).toBe("");
  });

  it("offers the add affordance to admins even when empty", () => {
    role = "admin";
    data = { series_id: "s2", relationships: [], chain: [] };
    const html = render();
    expect(html).toContain("Add related series");
    expect(html).toContain("No related series yet");
  });

  it("renders the chain in reading order with the current series marked", () => {
    role = "user";
    data = full;
    const html = render();
    expect(html).toContain("Reading order");
    const i1 = html.indexOf("Saga Vol 1");
    const i2 = html.indexOf("Saga Vol 2");
    const i3 = html.indexOf("Saga Vol 3");
    expect(i1).toBeGreaterThan(-1);
    expect(i1).toBeLessThan(i2);
    expect(i2).toBeLessThan(i3);
    expect(html).toContain('aria-current="true"');
    expect(html).toContain("This series");
    // Direct relationships grouped under their kind labels.
    expect(html).toContain("Sequel of");
    expect(html).toContain("Collected in");
    expect(html).toContain("Suggested");
    expect(html).toContain('href="/series/saga-omnibus"');
    // No admin affordances for a reader.
    expect(html).not.toContain("Add related series");
    expect(html).not.toContain("Remove ");
  });

  it("shows remove buttons to admins", () => {
    role = "admin";
    data = full;
    const html = render();
    expect(html).toContain('aria-label="Remove sequel of Saga Vol 1"');
  });
});

describe("helpers", () => {
  it("groups by kind in canonical order", () => {
    const groups = groupRelationships(full.relationships);
    expect(groups.map((g) => g.kind)).toEqual([
      "sequel_of",
      "prequel_of",
      "collected_in",
    ]);
  });

  it("captions chain positions", () => {
    expect(chainCaption(-2)).toBe("Read before");
    expect(chainCaption(0)).toBe("This series");
    expect(chainCaption(3)).toBe("Read after");
  });
});
