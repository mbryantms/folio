// @vitest-environment jsdom
/**
 * <SeriesSuggestedRelationships> — WP-7.3 admin chips on the series page.
 * Chips read from this series' point of view (the inverse kind when the
 * series is the suggestion's `to` end), carry the reason in a tooltip,
 * and accept / reject in one click. Not rendered for non-admins (the
 * Related tab gates it) or when nothing is pending.
 */
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  RelationshipSuggestionView,
  SeriesRelationshipsResp,
  SeriesView,
} from "@/lib/api/types";

const m = vi.hoisted(() => ({
  role: "admin" as "admin" | "user",
  items: [] as RelationshipSuggestionView[],
  hasNextPage: false,
  enabled: [] as boolean[],
  accept: vi.fn(),
  reject: vi.fn(),
}));

vi.mock("@/lib/api/queries", () => ({
  useRelationshipKinds: () => ({ data: undefined }),
  useMe: () => ({ data: { role: m.role } }),
  useSeriesRelationships: () => ({
    data: {
      series_id: "this",
      relationships: [],
      arcs: [],
      chain: [],
    } satisfies SeriesRelationshipsResp,
    isLoading: false,
  }),
  useSeriesListInfinite: () => ({ data: undefined, isLoading: false }),
  useSeriesRelationshipSuggestions: (_slug: string, enabled: boolean) => {
    m.enabled.push(enabled);
    return {
      data: { pages: [{ items: m.items, next_cursor: null }] },
      isLoading: false,
      error: null,
      hasNextPage: m.hasNextPage,
      isFetchingNextPage: false,
      fetchNextPage: vi.fn(),
    };
  },
}));
vi.mock("@/lib/api/mutations", () => ({
  useCreateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useDeleteSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useUpdateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useAcceptRelationshipSuggestion: () => ({
    mutate: m.accept,
    isPending: false,
  }),
  useRejectRelationshipSuggestion: () => ({
    mutate: m.reject,
    isPending: false,
  }),
}));

import { SeriesRelatedSection } from "@/components/library/SeriesRelatedSection";
import {
  SeriesSuggestedRelationships,
  fromPerspective,
} from "@/components/library/SeriesSuggestedRelationships";

function series(id: string, name: string, year: number): SeriesView {
  return {
    id,
    slug: `${name.toLowerCase().replace(/\s+/g, "-")}-${year}`,
    name,
    year,
    library_id: "lib",
    status: "continuing",
    language_code: "en",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    cover_url: null,
  } as SeriesView;
}

const self = series("this", "Daredevil", 2014);
const older = series("older", "Daredevil", 2011);
const omnibus = series("omni", "Daredevil Omnibus", 2020);

function sug(
  id: string,
  from: SeriesView,
  to: SeriesView,
  kind: RelationshipSuggestionView["kind"],
  label: string,
  inverse: RelationshipSuggestionView["kind"],
  inverseLabel: string,
): RelationshipSuggestionView {
  return {
    id,
    from_series: from,
    to_series: to,
    kind,
    kind_label: label,
    inverse_kind: inverse,
    inverse_kind_label: inverseLabel,
    confidence: 0.9,
    bucket: "high",
    reason: `reason ${id}`,
    evidence: {
      sources: [],
    } as unknown as RelationshipSuggestionView["evidence"],
    status: "pending",
    accepted_kind: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    reviewed_at: null,
    reviewed_by: null,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  m.role = "admin";
  m.hasNextPage = false;
  m.enabled.length = 0;
  m.items = [
    // This series is the subject: "Sequel to Daredevil (2011)".
    sug(
      "g1",
      self,
      older,
      "sequel_of",
      "Sequel to",
      "has_sequel",
      "Has sequel",
    ),
    // This series is the object of "Omnibus collects Daredevil": reads as
    // "Collected in Daredevil Omnibus".
    sug(
      "g2",
      omnibus,
      self,
      "collects",
      "Collects",
      "collected_in",
      "Collected in",
    ),
  ];
});

describe("fromPerspective", () => {
  it("inverts the kind when the series is the `to` end", () => {
    expect(fromPerspective(m.items[0]!, "this")).toMatchObject({
      kind: "sequel_of",
      label: "Sequel to",
      other: { id: "older" },
    });
    expect(fromPerspective(m.items[1]!, "this")).toMatchObject({
      kind: "collected_in",
      label: "Collected in",
      other: { id: "omni" },
    });
  });

  it("reads an arc tie-in's target from `to_arc` (WP-7.6)", () => {
    const arc = {
      ...sug(
        "g3",
        self,
        older,
        "tie_in_to",
        "Prelude to",
        "has_tie_in",
        "Has prelude",
      ),
      to_series: null,
      to_arc: { id: "arc1", slug: "secret-wars", name: "Secret Wars" },
      qualifier: "prelude" as const,
    };
    expect(fromPerspective(arc, "this")).toMatchObject({
      kind: "tie_in_to",
      label: "Prelude to",
      other: { id: "arc1", name: "Secret Wars", href: "/arcs/secret-wars" },
    });
  });
});

describe("<SeriesSuggestedRelationships>", () => {
  it("renders one chip per pending suggestion with accept / reject", () => {
    render(
      <SeriesSuggestedRelationships
        seriesSlug="daredevil-2014"
        seriesId="this"
      />,
    );
    expect(screen.getByText("Suggested")).toBeTruthy();
    const accept = screen.getByRole("button", {
      name: "Accept: Sequel to Daredevil (2011)",
    });
    fireEvent.click(accept);
    expect(m.accept).toHaveBeenCalledWith({ id: "g1" });
    fireEvent.click(
      screen.getByRole("button", {
        name: "Reject: Collected in Daredevil Omnibus (2020)",
      }),
    );
    expect(m.reject).toHaveBeenCalledWith({ id: "g2" });
    expect(
      screen
        .getByRole("link", { name: /Daredevil Omnibus/ })
        .getAttribute("href"),
    ).toBe("/series/daredevil-omnibus-2020");
  });

  it("renders nothing when nothing is pending", () => {
    m.items = [];
    const { container } = render(
      <SeriesSuggestedRelationships
        seriesSlug="daredevil-2014"
        seriesId="this"
      />,
    );
    expect(container.innerHTML).toBe("");
  });

  it("walks the next page with Show more", () => {
    m.hasNextPage = true;
    render(
      <SeriesSuggestedRelationships
        seriesSlug="daredevil-2014"
        seriesId="this"
      />,
    );
    expect(screen.getByRole("button", { name: "Show more" })).toBeTruthy();
  });

  it("is admin-only inside the Related block", () => {
    m.role = "user";
    const { container } = render(
      <SeriesRelatedSection seriesSlug="daredevil-2014" seriesId="this" />,
    );
    // WP-7.7: inside the Related tab a reader sees the empty state, never
    // the suggestion chips (and the suggestions query never runs).
    expect(container.textContent).toContain("No related series linked yet");
    expect(screen.queryByTestId("suggested-relationships")).toBeNull();
    expect(m.enabled).toEqual([]);
    m.role = "admin";
    render(
      <SeriesRelatedSection seriesSlug="daredevil-2014" seriesId="this" />,
    );
    expect(screen.getByTestId("suggested-relationships")).toBeTruthy();
  });
});
