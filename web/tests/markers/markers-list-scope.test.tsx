// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const infinite = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
}));

vi.mock("@/lib/api/queries", () => ({
  useMarkersInfinite: (filters: unknown) => {
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
  useMarkerCount: () => ({ data: { total: 0 } }),
  useMarkerTags: () => ({ data: { items: [] } }),
}));

vi.mock("@/lib/api/mutations", () => {
  const m = () => ({ mutate: vi.fn(), isPending: false });
  return {
    useBulkDeleteMarkers: m,
    useCreateMarker: m,
    useDeleteMarker: m,
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
    expect(screen.queryByRole("button", { name: /export notes/i })).toBeNull();
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
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe(
      "Bookmarks",
    );
    expect(screen.getByRole("button", { name: /export notes/i })).toBeTruthy();
  });
});
