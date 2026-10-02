// @vitest-environment jsdom
/**
 * <RelationshipSuggestionsPanel> — WP-7.3 review queue. Filters drive the
 * (mocked) infinite query's server params; rows show both series, the
 * kind, confidence and reason with expandable evidence; per-row accept /
 * reject / reopen and the bulk paths call the right mutations.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  RelationshipSuggestionView,
  SeriesView,
  SuggestionStatus,
} from "@/lib/api/types";

const m = vi.hoisted(() => ({
  filters: [] as Array<{
    status: string;
    bucket: string | null;
    libraryId: string | null;
  }>,
  accept: vi.fn(),
  reject: vi.fn(),
  reopen: vi.fn(),
  bulkAccept: vi.fn(),
  bulkReject: vi.fn(),
  run: vi.fn(),
}));

function series(id: string, name: string, year: number): SeriesView {
  return {
    id,
    slug: name.toLowerCase().replace(/\s+/g, "-") + `-${year}`,
    name,
    year,
    library_id: "lib-1",
    status: "continuing",
    language_code: "en",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    cover_url: null,
  } as SeriesView;
}

function suggestion(
  id: string,
  status: SuggestionStatus,
  confidence = 0.9,
): RelationshipSuggestionView {
  return {
    id,
    from_series: series(`${id}-a`, "Daredevil", 2014),
    to_series: series(`${id}-b`, "Daredevil", 2011),
    kind: "sequel_of",
    kind_label: "Sequel of",
    confidence,
    bucket: confidence >= 0.8 ? "high" : "medium",
    reason: "Daredevil (2014) follows Daredevil (2011) — next volume",
    evidence: {
      sources: [
        {
          source: "name_continuation",
          confidence: 0.9,
          reason: "next volume",
          volumes: [3, 4],
        },
      ],
    } as unknown as RelationshipSuggestionView["evidence"],
    status,
    accepted_kind: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    reviewed_at: status === "pending" ? null : "2026-01-02T00:00:00Z",
    reviewed_by: null,
  };
}

vi.mock("@/lib/api/queries", () => ({
  useLibraryList: () => ({
    data: [{ id: "lib-1", name: "Comics", slug: "comics" }],
    isLoading: false,
  }),
  useRelationshipSuggestionsInfinite: (f: {
    status: string;
    bucket: string | null;
    libraryId: string | null;
  }) => {
    m.filters.push(f);
    const status = (
      f.status === "all" ? "pending" : f.status
    ) as SuggestionStatus;
    return {
      data: {
        pages: [
          {
            items: [suggestion("s1", status), suggestion("s2", status, 0.6)],
            next_cursor: null,
            total: 2,
            bucket_counts: { high: 3, medium: 2, low: 1 },
          },
        ],
      },
      isLoading: false,
      error: null,
      hasNextPage: false,
      isFetchingNextPage: false,
      fetchNextPage: vi.fn(),
    };
  },
}));
vi.mock("@/lib/api/mutations", () => ({
  useAcceptRelationshipSuggestion: () => ({
    mutate: m.accept,
    isPending: false,
  }),
  useRejectRelationshipSuggestion: () => ({
    mutate: m.reject,
    isPending: false,
  }),
  useReopenRelationshipSuggestion: () => ({
    mutate: m.reopen,
    isPending: false,
  }),
  useBulkAcceptRelationshipSuggestions: () => ({
    mutate: m.bulkAccept,
    isPending: false,
  }),
  useBulkRejectRelationshipSuggestions: () => ({
    mutate: m.bulkReject,
    isPending: false,
  }),
  useRunRelationshipSuggestions: () => ({ mutate: m.run, isPending: false }),
}));

import {
  RelationshipSuggestionsPanel,
  evidenceSources,
  sourceLabel,
} from "@/components/admin/relationships/RelationshipSuggestionsPanel";

class NoopObserver {
  observe() {}
  disconnect() {}
  unobserve() {}
}
vi.stubGlobal("IntersectionObserver", NoopObserver);

beforeEach(() => {
  vi.clearAllMocks();
  m.filters.length = 0;
});

const lastFilters = () => m.filters[m.filters.length - 1]!;

describe("<RelationshipSuggestionsPanel>", () => {
  it("renders pending suggestions with counts, kind, confidence and reason", () => {
    render(<RelationshipSuggestionsPanel />);
    expect(lastFilters()).toEqual({
      status: "pending",
      bucket: null,
      libraryId: null,
    });
    const rows = screen.getAllByTestId("relationship-suggestion");
    expect(rows).toHaveLength(2);
    const row = within(rows[0]!);
    expect(row.getAllByText("Daredevil")).toHaveLength(2);
    expect(row.getByText("Sequel of")).toBeTruthy();
    expect(row.getByText("90%")).toBeTruthy();
    expect(row.getByText(/next volume$/)).toBeTruthy();
    // Bucket pills carry the first page's counts.
    expect(screen.getByRole("button", { name: /^High/ }).textContent).toContain(
      "3",
    );
    expect(
      screen.getByRole("button", { name: /Accept all high-confidence \(3\)/ }),
    ).toBeTruthy();
  });

  it("drives status, bucket and library as server params", () => {
    render(<RelationshipSuggestionsPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Rejected" }));
    expect(lastFilters().status).toBe("rejected");
    fireEvent.click(screen.getByRole("button", { name: /^Medium/ }));
    expect(lastFilters().bucket).toBe("medium");
    fireEvent.change(screen.getByLabelText("Library"), {
      target: { value: "lib-1" },
    });
    expect(lastFilters()).toEqual({
      status: "rejected",
      bucket: "medium",
      libraryId: "lib-1",
    });
  });

  it("accepts and rejects a pending row", () => {
    render(<RelationshipSuggestionsPanel />);
    const row = within(screen.getAllByTestId("relationship-suggestion")[0]!);
    fireEvent.click(row.getByRole("button", { name: "Accept" }));
    expect(m.accept).toHaveBeenCalledWith({ id: "s1" });
    fireEvent.click(row.getByRole("button", { name: "Reject" }));
    expect(m.reject).toHaveBeenCalledWith({ id: "s1" });
    expect(row.getByRole("button", { name: "Edit kind" })).toBeTruthy();
    expect(row.queryByRole("button", { name: "Reopen" })).toBeNull();
  });

  it("offers reopen in the rejected view", () => {
    render(<RelationshipSuggestionsPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Rejected" }));
    const row = within(screen.getAllByTestId("relationship-suggestion")[0]!);
    expect(row.queryByRole("button", { name: "Accept" })).toBeNull();
    fireEvent.click(row.getByRole("button", { name: "Reopen" }));
    expect(m.reopen).toHaveBeenCalledWith({ id: "s1" });
  });

  it("expands the evidence", () => {
    render(<RelationshipSuggestionsPanel />);
    const row = within(screen.getAllByTestId("relationship-suggestion")[0]!);
    fireEvent.click(row.getByRole("button", { name: /Evidence \(1 source\)/ }));
    expect(row.getByText(/Name continuation/)).toBeTruthy();
    expect(row.getByText("3, 4")).toBeTruthy();
  });

  it("accepts all high-confidence suggestions behind a confirm", () => {
    render(<RelationshipSuggestionsPanel />);
    fireEvent.change(screen.getByLabelText("Library"), {
      target: { value: "lib-1" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /Accept all high-confidence/ }),
    );
    expect(m.bulkAccept).not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Accept all" }));
    expect(m.bulkAccept).toHaveBeenCalledWith({
      bucket: "high",
      library_id: "lib-1",
    });
  });

  it("bulk-accepts the selected rows", () => {
    render(<RelationshipSuggestionsPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Select…" }));
    const boxes = screen.getAllByRole("checkbox");
    fireEvent.click(boxes[0]!);
    fireEvent.click(boxes[1]!);
    const accept = screen
      .getAllByRole("button", { name: /^Accept$/ })
      .find((b) => b.closest(".selection-toolbar-wrap"));
    fireEvent.click(accept!);
    expect(m.bulkAccept).toHaveBeenCalledWith(
      { ids: ["s1", "s2"] },
      expect.anything(),
    );
  });

  it("runs the engine for the chosen library", () => {
    render(<RelationshipSuggestionsPanel />);
    fireEvent.click(screen.getByRole("button", { name: /Run now/ }));
    expect(m.run).toHaveBeenCalledWith({ libraryId: null });
    fireEvent.change(screen.getByLabelText("Library"), {
      target: { value: "lib-1" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Run now for Comics/ }));
    expect(m.run).toHaveBeenLastCalledWith({ libraryId: "lib-1" });
  });
});

describe("evidence helpers", () => {
  it("tolerates odd shapes and labels sources", () => {
    expect(evidenceSources(null)).toEqual([]);
    expect(evidenceSources({ sources: "nope" })).toEqual([]);
    expect(evidenceSources({ sources: [{ source: "x" }, 3] })).toEqual([
      { source: "x" },
    ]);
    expect(sourceLabel("character_density")).toBe("Character density");
  });
});
