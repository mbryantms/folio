/**
 * <SeriesRelatedSection> — WP-7.1, in the Related tab since WP-7.7.
 * Static-markup smoke over the mocked relationships query: renders the
 * reading-order chain in position order with this series highlighted,
 * groups direct relationships by kind, lists arc edges under "Part of
 * event" with their role, sizes covers to `coverWidth`, gates the admin
 * affordances (add / edit / remove), shows an empty state, and shows a
 * skeleton — never the raw group key — before the kind catalogue loads.
 * WP-7.8: external ("not in your library") links in their kind's group.
 */
import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import type {
  RelationshipCatalogue,
  SeriesRelationshipsResp,
  SeriesView,
} from "@/lib/api/types";

let role: "admin" | "user" = "user";
let data: SeriesRelationshipsResp | undefined;
let catalogue: RelationshipCatalogue | undefined;

vi.mock("@/lib/api/queries", () => ({
  useRelationshipKinds: () => ({ data: catalogue }),
  useMe: () => ({ data: { role } }),
  useSeriesRelationships: () => ({ data, isLoading: false }),
  useSeriesListInfinite: () => ({ data: undefined, isLoading: false }),
  useSeriesRelationshipSuggestions: () => ({
    data: undefined,
    isLoading: false,
  }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useCreateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteExternalRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useCreateExternalRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useUpdateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useAcceptRelationshipSuggestion: () => ({
    mutate: vi.fn(),
    isPending: false,
  }),
  useRejectRelationshipSuggestion: () => ({
    mutate: vi.fn(),
    isPending: false,
  }),
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
      kind_label: "Sequel to",
      group: "story",
      source: "manual",
      confidence: null,
      created_at: "2026-01-01T00:00:00Z",
      series: vol1,
    },
    {
      id: "r2",
      kind: "collected_in",
      kind_label: "Collected in",
      group: "editions",
      to_range: "1-6",
      coverage: "partial",
      note: "Deluxe",
      source: "suggested",
      confidence: 0.9,
      created_at: "2026-01-01T00:00:00Z",
      series: omnibus,
    },
    {
      id: "r3",
      kind: "continued_by",
      kind_label: "Continued by",
      group: "publication",
      qualifier: "relaunch",
      qualifier_label: "Relaunch",
      source: "manual",
      confidence: null,
      created_at: "2026-01-01T00:00:00Z",
      series: vol3,
    },
  ],
  arcs: [
    {
      id: "a1",
      kind: "tie_in_to",
      kind_label: "Prelude to",
      group: "story",
      qualifier: "prelude",
      qualifier_label: "Prelude",
      source: "manual",
      created_at: "2026-01-01T00:00:00Z",
      arc: {
        id: "arc1",
        slug: "war-for-the-realms",
        name: "War for the Realms",
      },
    },
  ],
  chain: [
    { position: -1, series: vol1 },
    { position: 0, series: vol2 },
    { position: 1, series: vol3 },
  ],
  external: [],
};

const CATALOGUE = {
  groups: [
    { group: "story", label: "Story" },
    { group: "publication", label: "Publication history" },
    { group: "editions", label: "Editions & contents" },
    { group: "advanced", label: "Advanced" },
  ],
  kinds: [],
} as unknown as RelationshipCatalogue;

function render(coverWidth?: number) {
  return renderToStaticMarkup(
    createElement(SeriesRelatedSection, {
      seriesSlug: "saga-vol-2",
      seriesId: "s2",
      coverWidth,
    }),
  );
}

describe("<SeriesRelatedSection>", () => {
  it("shows an empty state to readers when there are no relationships", () => {
    role = "user";
    catalogue = CATALOGUE;
    data = {
      series_id: "s2",
      relationships: [],
      arcs: [],
      chain: [],
      external: [],
    };
    const html = render();
    expect(html).toContain("No related series linked yet");
    expect(html).not.toContain("Add relationship");
  });

  it("offers the add affordance to admins even when empty", () => {
    role = "admin";
    catalogue = CATALOGUE;
    data = {
      series_id: "s2",
      relationships: [],
      arcs: [],
      chain: [],
      external: [],
    };
    const html = render();
    expect(html).toContain("Add relationship");
    expect(html).toContain("No related series yet");
  });

  it("sizes every cover to the grid's column width", () => {
    role = "user";
    catalogue = CATALOGUE;
    data = full;
    const html = render(213.5);
    // 3 chain cards + 3 relationship cards, all at the given width.
    expect(html.match(/width:213\.5px/g)?.length).toBe(6);
  });

  it("shows a skeleton, not the raw group key, before the catalogue loads", () => {
    role = "user";
    catalogue = undefined;
    data = full;
    const html = render();
    expect(html).toContain('data-testid="relationship-group-loading"');
    expect(html).not.toContain(">Editions<");
    expect(html).not.toContain(">Publication<");
  });

  it("renders the chain in reading order with the current series marked", () => {
    role = "user";
    catalogue = CATALOGUE;
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
    // Direct relationships grouped under UI groups and kind labels.
    expect(html).toContain("Story");
    expect(html).toContain("Publication history");
    expect(html).toContain("Editions &amp; contents");
    expect(html).toContain("Sequel to");
    expect(html).toContain("Collected in");
    expect(html).toContain("Continued by");
    // Scope as secondary text.
    expect(html).toContain("#1-6");
    expect(html).toContain("partial coverage");
    expect(html).toContain("Deluxe");
    expect(html).toContain("Relaunch");
    // Arc tie-ins: "Part of event", linking the arc with its role.
    expect(html).toContain("Part of event");
    expect(html).toContain("Prelude to");
    expect(html).toContain('href="/arcs/war-for-the-realms"');
    expect(html).toContain("Suggested");
    expect(html).toContain('href="/series/saga-omnibus"');
    // No admin affordances for a reader.
    expect(html).not.toContain("Add relationship");
    expect(html).not.toContain("Remove ");
    expect(html).not.toContain("Edit ");
  });

  it("shows edit + remove buttons to admins, arcs included", () => {
    role = "admin";
    catalogue = CATALOGUE;
    data = full;
    const html = render();
    expect(html).toContain('aria-label="Edit sequel to Saga Vol 1"');
    expect(html).toContain('aria-label="Remove sequel to Saga Vol 1"');
    expect(html).toContain('aria-label="Edit prelude to War for the Realms"');
    expect(html).toContain('aria-label="Remove prelude to War for the Realms"');
  });
});

const EXTERNAL = [
  {
    id: "x1",
    kind: "continued_by",
    kind_label: "Continued by",
    group: "publication",
    source: "metron",
    source_label: "Metron",
    provider_series_id: "2311",
    name: "Saga",
    year: 2018,
    url: "https://metron.cloud/series/2311/",
    set_by: "provider",
    confidence: 0.6,
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "x2",
    kind: "has_annual",
    kind_label: "Has annual",
    group: "publication",
    source: "comicvine",
    source_label: "ComicVine",
    provider_series_id: "9",
    name: "Saga Annual",
    year: null,
    url: null,
    set_by: "user",
    confidence: null,
    created_at: "2026-01-01T00:00:00Z",
    local_series: { id: "s9", slug: "saga-annual", name: "Saga Annual" },
  },
] as unknown as SeriesRelationshipsResp["external"];

describe("<SeriesRelatedSection> external links (WP-7.8)", () => {
  it("lists them in their kind's group as muted text rows with a provider link", () => {
    role = "user";
    catalogue = CATALOGUE;
    data = { ...full, external: EXTERNAL };
    const html = render(100);
    // Same "Continued by" heading as the local Vol 3 card, once.
    expect(html.match(/>Continued by</g)?.length).toBe(1);
    expect(html).toContain("Saga (2018)");
    expect(html).toContain("— not in your library");
    expect(html).toContain('href="https://metron.cloud/series/2311/"');
    expect(html).toContain('target="_blank"');
    // A resolved user link points at the local series instead.
    expect(html).toContain('href="/series/saga-annual"');
    expect(html).toContain("in your library");
    // Compact rows: no extra covers (6 cards as before).
    expect(html.match(/width:100px/g)?.length).toBe(6);
    expect(html.match(/data-testid="external-relationship"/g)?.length).toBe(2);
    // Readers can't remove them.
    expect(html).not.toContain("Remove continued by Saga (2018)");
  });

  it("gives admins a remove button and counts them against the empty state", () => {
    role = "admin";
    catalogue = CATALOGUE;
    data = {
      series_id: "s2",
      relationships: [],
      arcs: [],
      chain: [],
      external: EXTERNAL,
    };
    const html = render();
    expect(html).not.toContain("No related series yet");
    expect(html).toContain("Publication history");
    expect(html).toContain('aria-label="Remove continued by Saga (2018)"');
  });
});

describe("helpers", () => {
  it("groups by UI group, then kind, in catalogue order", () => {
    const groups = groupRelationships(full.relationships);
    expect(groups.map((g) => g.group)).toEqual([
      "story",
      "publication",
      "editions",
    ]);
    expect(groups.flatMap((g) => g.kinds.map((k) => k.kind))).toEqual([
      "sequel_of",
      "continued_by",
      "collected_in",
    ]);
    // The catalogue's group labels and kind order win once loaded.
    const catalogue = {
      groups: [
        { group: "editions", label: "Editions & contents" },
        { group: "story", label: "Story" },
        { group: "publication", label: "Publication history" },
      ],
      kinds: [],
    } as unknown as RelationshipCatalogue;
    const withCatalogue = groupRelationships(full.relationships, catalogue);
    expect(withCatalogue.map((g) => g.label)).toEqual([
      "Editions & contents",
      "Story",
      "Publication history",
    ]);
  });

  it("puts external links in the slot of their kind label", () => {
    const groups = groupRelationships(full.relationships, undefined, EXTERNAL);
    const publication = groups.find((g) => g.group === "publication")!;
    const continued = publication.kinds.find(
      (k) => k.label === "Continued by",
    )!;
    expect(continued.items.map((r) => r.id)).toEqual(["r3"]);
    expect(continued.external.map((r) => r.id)).toEqual(["x1"]);
    const annual = publication.kinds.find((k) => k.label === "Has annual")!;
    expect(annual.items).toEqual([]);
    expect(annual.external.map((r) => r.id)).toEqual(["x2"]);
  });

  it("captions chain positions", () => {
    expect(chainCaption(-2)).toBe("Read before");
    expect(chainCaption(0)).toBe("This series");
    expect(chainCaption(3)).toBe("Read after");
  });
});

describe("<SeriesRelatedSection> coverage links (coverage tie-ins)", () => {
  it("shows a coverage link's note next to the not-in-library row", () => {
    role = "user";
    catalogue = CATALOGUE;
    data = {
      ...full,
      external: [
        {
          ...EXTERNAL[0],
          id: "x3",
          provider_series_id: "1713",
          name: "Fantastic Four",
          year: 2012,
          url: "https://metron.cloud/series/1713/",
          confidence: 0.7,
          note: "Has #612–645",
        },
      ] as unknown as SeriesRelationshipsResp["external"],
    };
    const html = render(100);
    expect(html).toContain("Fantastic Four (2012)");
    expect(html).toContain("— not in your library");
    expect(html).toContain('data-testid="external-relationship-note"');
    expect(html).toContain("Has #612–645");
  });
});
