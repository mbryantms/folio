// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const infinite = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
}));

vi.mock("@/lib/api/queries", () => ({
  useBookmarksFeedInfinite: (filters: unknown) => {
    infinite(filters);
    return {
      data: { pages: [{ items: [], next_cursor: null }] },
      isLoading: false,
      isError: false,
      hasNextPage: false,
      isFetchingNextPage: false,
      fetchNextPage: vi.fn(),
    };
  },
  useBookmarkCounts: () => ({
    data: {
      total: 0,
      all: 0,
      by_kind: {
        bookmark: 0,
        note: 0,
        favorite: 0,
        highlight: 0,
        closing_note: 0,
      },
    },
  }),
  useBookmarkSeries: () => ({ data: { items: [] } }),
  useMarkerTags: () => ({ data: { items: [] } }),
}));

vi.mock("@/lib/api/mutations", () => {
  const m = () => ({ mutate: vi.fn(), isPending: false });
  return {
    useBulkDeleteMarkers: m,
    useCreateMarker: m,
    useDeleteMarker: m,
    useRestoreMarkers: m,
    useUpdateMarker: m,
  };
});

const { MarkersList } = await import("@/components/markers/MarkersList");

describe("MarkersList scope (WP-5.2)", () => {
  beforeEach(() => infinite.mockClear());

  it("scopes the feed to a series server-side and drops the page chrome", () => {
    render(<MarkersList scope={{ kind: "series", seriesId: "s-1" }} />);
    expect(infinite).toHaveBeenCalledWith(
      expect.objectContaining({ series_id: "s-1", issue_id: undefined }),
    );
    // Embedded in a tab: no page-level h1 and no export menu.
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    expect(screen.queryByRole("button", { name: /more actions/i })).toBeNull();
    expect(screen.getByText(/no markers in this series/i)).toBeTruthy();
  });

  it("scopes the feed to an issue", () => {
    render(<MarkersList scope={{ kind: "issue", issueId: "i-1" }} />);
    expect(infinite).toHaveBeenCalledWith(
      expect.objectContaining({ issue_id: "i-1", series_id: undefined }),
    );
    expect(screen.getByText(/no markers in this issue/i)).toBeTruthy();
  });

  it("keeps the global /bookmarks page unscoped", () => {
    render(<MarkersList />);
    expect(infinite).toHaveBeenCalledWith(
      expect.objectContaining({ issue_id: undefined, series_id: undefined }),
    );
    // Two h1s render in jsdom: the desktop PageHeader's and the phone
    // bar's (one is hidden per breakpoint by CSS). Both carry the title.
    const headings = screen.getAllByRole("heading", { level: 1 });
    expect(headings.length).toBe(2);
    for (const h of headings) expect(h.textContent).toBe("Bookmarks");
    expect(
      screen.getAllByRole("button", { name: /more actions/i }).length,
    ).toBeGreaterThan(0);
  });
});
