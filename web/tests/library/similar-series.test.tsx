/**
 * WP-7.4 similar series — the "because" formatter and the opt-in
 * `similar_series` system rail body (loading skeletons / hide-when-empty /
 * one `SimilarSeriesCard` per item). Node env, element-tree walk, same
 * idiom as `recent-issues-rail.test.tsx`.
 */
import { describe, expect, it, vi } from "vitest";
import type * as React from "react";

const mockUseSimilarRail = vi.fn();
vi.mock("@/lib/api/queries", () => ({
  useContinueReading: () => ({ isLoading: false, data: { items: [] } }),
  useOnDeck: () => ({ isLoading: false, data: { items: [] } }),
  useRecentIssues: () => ({ isLoading: false, data: { items: [] } }),
  useSimilarRailInfinite: (...args: unknown[]) => mockUseSimilarRail(...args),
}));
vi.mock("@/lib/api/mutations", () => ({
  useDismissRailItem: () => ({ mutate: vi.fn() }),
}));

import { SeriesCardSkeleton } from "@/components/library/SeriesCard";
import { SimilarSeriesCard } from "@/components/library/SimilarSeriesCard";
import {
  SimilarSeriesRailBody,
  useSystemRailIsEmpty,
} from "@/components/saved-views/system-rails";
import type { SimilarReason, SimilarSeriesItem } from "@/lib/api/types";
import { formatBecause, reasonLabel } from "@/lib/similar";

function item(id: string, because: SimilarReason[] = []): SimilarSeriesItem {
  return {
    series: {
      id,
      name: `Series ${id}`,
      slug: id,
    } as SimilarSeriesItem["series"],
    score: 3,
    because,
  };
}

function collectByType(node: React.ReactNode, type: unknown): unknown[] {
  const found: unknown[] = [];
  const stack: React.ReactNode[] = [node];
  while (stack.length) {
    const cur = stack.shift();
    if (Array.isArray(cur)) {
      stack.push(...cur);
      continue;
    }
    if (!cur || typeof cur !== "object" || !("type" in cur)) continue;
    const el = cur as React.ReactElement<{ children?: React.ReactNode }>;
    if (el.type === type) found.push(el);
    if (el.props && el.props.children) stack.push(el.props.children);
  }
  return found;
}

const itemStyle: React.CSSProperties = { width: "160px" };

describe("because formatting", () => {
  it("labels each kind the way the rail caption reads", () => {
    expect(
      reasonLabel({
        kind: "creator",
        role: "writer",
        name: "Ed Brubaker",
        weight: 2,
      }),
    ).toBe("writer Ed Brubaker");
    expect(
      reasonLabel({
        kind: "creator",
        role: "cover_artist",
        name: "Steve Epting",
        weight: 1,
      }),
    ).toBe("cover artist Steve Epting");
    expect(
      reasonLabel({ kind: "character", name: "Bucky Barnes", weight: 1 }),
    ).toBe("character Bucky Barnes");
    expect(
      reasonLabel({
        kind: "relationship",
        role: "sequel_of",
        name: "Daredevil",
        weight: 6,
      }),
    ).toBe("sequel of Daredevil");
  });

  it("joins the strongest reasons in server order", () => {
    const reasons: SimilarReason[] = [
      { kind: "creator", role: "writer", name: "Ed Brubaker", weight: 2 },
      { kind: "character", name: "Bucky Barnes", weight: 1.5 },
      { kind: "arc", name: "Winter Soldier", weight: 1.2 },
      { kind: "genre", name: "Superhero", weight: 0.1 },
    ];
    expect(formatBecause(reasons)).toBe(
      "writer Ed Brubaker, character Bucky Barnes, arc Winter Soldier",
    );
    expect(formatBecause([])).toBe("");
  });
});

describe("SimilarSeriesRailBody", () => {
  it("renders skeletons while loading", () => {
    mockUseSimilarRail.mockReturnValue({ isLoading: true, data: undefined });
    const tree = SimilarSeriesRailBody({ itemStyle });
    expect(collectByType(tree, SeriesCardSkeleton).length).toBeGreaterThan(0);
  });

  it("returns null when there is nothing to suggest", () => {
    mockUseSimilarRail.mockReturnValue({
      isLoading: false,
      data: { pages: [{ seed: null, items: [], next_cursor: null }] },
    });
    expect(SimilarSeriesRailBody({ itemStyle })).toBeNull();
  });

  it("renders one SimilarSeriesCard per first-page item", () => {
    mockUseSimilarRail.mockReturnValue({
      isLoading: false,
      data: {
        pages: [
          {
            seed: { id: "s0", name: "Seed", slug: "seed" },
            items: [item("a"), item("b")],
            next_cursor: "x",
          },
        ],
      },
    });
    const tree = SimilarSeriesRailBody({ itemStyle });
    const cards = collectByType(tree, SimilarSeriesCard) as Array<
      React.ReactElement<{ item: SimilarSeriesItem }>
    >;
    expect(cards.map((c) => c.props.item.series.id)).toEqual(["a", "b"]);
  });

  it("useSystemRailIsEmpty gates on the similar_series key", () => {
    mockUseSimilarRail.mockClear();
    mockUseSimilarRail.mockReturnValue({
      isLoading: false,
      data: { pages: [{ seed: null, items: [], next_cursor: null }] },
    });
    expect(useSystemRailIsEmpty("similar_series")).toBe(true);
    expect(mockUseSimilarRail).toHaveBeenLastCalledWith({ enabled: true });
    useSystemRailIsEmpty("on_deck");
    expect(mockUseSimilarRail).toHaveBeenLastCalledWith({ enabled: false });
  });
});
