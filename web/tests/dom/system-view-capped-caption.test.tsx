// @vitest-environment jsdom
/**
 * The Continue reading, On deck and Recently read "see all" pages fetch
 * an endpoint capped at `PROGRESS_RAIL_CAP` items with no cursor. A full
 * response is captioned "Most recent 24" — never an "N items" total that
 * reads as the whole list. A shorter response is complete, so its count
 * stays.
 */
import { render, screen } from "@testing-library/react";
import type * as Queries from "@/lib/api/queries";
import { describe, expect, it, vi } from "vitest";

const data = vi.hoisted(() => ({ items: [] as unknown[] }));

vi.mock("@/lib/api/queries", async (orig) => {
  const idle = { data: undefined, isLoading: false };
  const loaded = () => ({ data: { items: data.items }, isLoading: false });
  return {
    ...(await orig<typeof Queries>()),
    useContinueReading: (o?: { enabled?: boolean }) =>
      o?.enabled ? loaded() : idle,
    useOnDeck: (o?: { enabled?: boolean }) => (o?.enabled ? loaded() : idle),
    useRecentHistory: (o?: { enabled?: boolean }) =>
      o?.enabled ? loaded() : idle,
    useRandomPicks: () => idle,
    useOnThisDay: () => idle,
    useIssuesCrossListInfinite: () => idle,
    useSimilarRailInfinite: () => idle,
  };
});
vi.mock("@/components/library/ProgressIssueCard", () => ({
  ProgressIssueCard: () => <div data-testid="card" />,
  ProgressIssueCardSkeleton: () => null,
}));
vi.mock("@/components/library/OnDeckCard", () => ({
  OnDeckCard: () => <div data-testid="card" />,
  OnDeckCardSkeleton: () => null,
}));
vi.mock("@/components/library/IssueCard", () => ({
  IssueCard: () => null,
  IssueCardSkeleton: () => null,
}));
vi.mock("@/components/library/SeriesCard", () => ({
  SeriesCard: () => null,
  SeriesCardSkeleton: () => null,
}));
vi.mock("@/components/library/SimilarSeriesCard", () => ({
  SimilarSeriesCard: () => null,
}));
vi.mock("@/components/library/CardSizeOptions", () => ({
  CardSizeOptions: () => null,
  CardSizeFields: () => null,
}));
vi.mock("@/components/library/PhoneControlsBar", () => ({
  PhoneControlsBar: () => null,
}));
vi.mock("@/components/library/use-card-size", () => ({
  useCardSize: () => [180, () => {}],
}));

import { SystemViewDetail } from "@/components/saved-views/SystemViewDetail";
import { PROGRESS_RAIL_CAP } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

function view(system_key: string, name: string): SavedViewView {
  return {
    id: `system:${system_key}`,
    kind: "system",
    system_key,
    name,
    description: null,
  } as unknown as SavedViewView;
}

function items(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    kind: "series_next",
    issue: { id: `i${i}`, series_id: `s${i}` },
  }));
}

const PAGES = [
  ["continue_reading", "Continue reading"],
  ["on_deck", "On deck"],
  ["recent_history", "Recently read"],
] as const;

describe("capped system view pages", () => {
  it("caps at the server's 24", () => {
    expect(PROGRESS_RAIL_CAP).toBe(24);
  });

  for (const [key, name] of PAGES) {
    it(`${name}: a full response says "Most recent 24", no total`, () => {
      data.items = items(PROGRESS_RAIL_CAP);
      const { container } = render(<SystemViewDetail view={view(key, name)} />);
      expect(screen.getByText(`Most recent ${PROGRESS_RAIL_CAP}`)).toBeTruthy();
      expect(container.textContent).not.toMatch(/\d+ items?\b/);
      expect(screen.getAllByTestId("card")).toHaveLength(PROGRESS_RAIL_CAP);
    });

    it(`${name}: a short response is complete and keeps its count`, () => {
      data.items = items(3);
      render(<SystemViewDetail view={view(key, name)} />);
      expect(screen.getByText("3 items")).toBeTruthy();
      expect(screen.queryByText(/Most recent/)).toBeNull();
    });
  }
});
