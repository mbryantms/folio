// @vitest-environment jsdom
/**
 * `<IssueFilterViewDetail>` multi-select (audit 2026-11 N-O4): the
 * issue-level saved view gets the same `useSelection` + `SelectionToolbar`
 * wiring as the series twin. Select enters select mode, toggling cards
 * grows the count, the toolbar's Mark read / Mark unread call the bulk
 * progress mutation with the selected issue ids and leave select mode,
 * Done exits and hands focus back to the Select trigger. Cards are
 * stubbed: the card's own navigate-vs-toggle switch is covered by the
 * IssueCard tests; drag-reorder is not part of this surface.
 */
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { IssueSummaryView, SavedViewView } from "@/lib/api/types";

const issues = ["i1", "i2", "i3"].map(
  (id, n) =>
    ({
      id,
      series_id: "s1",
      series_slug: "saga",
      series_name: "Saga",
      number: String(n + 1),
      title: `Chapter ${n + 1}`,
      page_count: 24,
    }) as unknown as IssueSummaryView,
);

vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  useSavedViewIssueResultsInfinite: () => ({
    data: { pages: [{ items: issues, total: issues.length }] },
    isLoading: false,
    isError: false,
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: vi.fn(),
  }),
  useMe: () => ({ data: { role: "user" } }),
}));

const bulkMutate = vi.fn();
vi.mock("@/lib/api/mutations", async (importOriginal) => {
  const inert = () => ({ mutate: vi.fn(), isPending: false });
  return {
    ...(await importOriginal<Record<string, unknown>>()),
    useBulkMarkProgress: () => ({ mutate: bulkMutate, isPending: false }),
    useCreateSavedViewBatch: inert,
    useSidebarSavedView: inert,
  };
});
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
  usePathname: () => "/views/v1",
}));
vi.mock("sonner", () => ({
  toast: { info: vi.fn(), success: vi.fn(), error: vi.fn() },
}));
vi.mock("@/components/saved-views/EditFilterViewSheet", () => ({
  EditFilterViewSheet: () => null,
}));
// The header's ⋯ menu mounts the pin dialog (TanStack queries) eagerly.
vi.mock("@/components/saved-views/MultiPinDialog", () => ({
  MultiPinDialog: () => null,
}));
vi.mock("@/components/collections/BulkAddToCollectionDialog", () => ({
  BulkAddToCollectionDialog: ({ open }: { open: boolean }) =>
    open ? <div role="dialog" aria-label="Add to collection" /> : null,
}));
// A card is a button that reports the parent's select-mode props; the
// real card's navigate-vs-toggle branch has its own tests.
vi.mock("@/components/library/IssueCard", () => ({
  IssueCard: ({
    issue,
    selectMode,
    onEnterSelectMode,
  }: {
    issue: IssueSummaryView;
    selectMode?: {
      isActive: boolean;
      isSelected: boolean;
      onToggle: (ev?: React.MouseEvent) => void;
    };
    onEnterSelectMode?: (id: string) => void;
  }) => (
    <button
      type="button"
      data-testid={`card-${issue.id}`}
      aria-pressed={selectMode?.isSelected ?? false}
      data-select-active={selectMode?.isActive ? "true" : "false"}
      onClick={(ev) =>
        selectMode ? selectMode.onToggle(ev) : onEnterSelectMode?.(issue.id)
      }
    >
      {issue.title}
    </button>
  ),
  IssueCardSkeleton: () => null,
}));

import { IssueFilterViewDetail } from "@/components/saved-views/IssueFilterViewDetail";

const view: SavedViewView = {
  id: "v1",
  kind: "filter_issues",
  user_id: "u1",
  is_system: false,
  name: "Short unread reads",
  description: null,
  custom_year_start: null,
  custom_year_end: null,
  custom_tags: [],
  match_mode: "all",
  conditions: [],
  sort_field: "created_at",
  sort_order: "desc",
  result_limit: 12,
  cbl_list_id: null,
  pinned: false,
  pinned_position: null,
  show_in_sidebar: false,
  pinned_on_pages: [],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
} as SavedViewView;

const card = (id: string) => screen.getByTestId(`card-${id}`);
const toolbar = () => screen.queryByRole("toolbar");
// The toolbar lingers through its exit animation; "closed" counts as gone.
const toolbarClosed = () => {
  const t = toolbar();
  return t === null || t.getAttribute("data-state") === "closed";
};

describe("IssueFilterViewDetail multi-select", () => {
  beforeAll(() => {
    // The load-more sentinel observes itself on mount; jsdom has no IO.
    class IOStub {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    vi.stubGlobal("IntersectionObserver", IOStub);
  });
  beforeEach(() => bulkMutate.mockReset());

  it("Select enters select mode, toggling cards drives the toolbar count", () => {
    render(<IssueFilterViewDetail view={view} />);
    expect(toolbar()).toBeNull();
    expect(card("i1").getAttribute("data-select-active")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "Select" }));
    const bar = toolbar();
    expect(bar).not.toBeNull();
    expect(bar!.getAttribute("aria-label")).toBe("0 items selected");
    // Every card now toggles instead of navigating.
    for (const id of ["i1", "i2", "i3"]) {
      expect(card(id).getAttribute("data-select-active")).toBe("true");
    }

    fireEvent.click(card("i1"));
    fireEvent.click(card("i3"));
    expect(card("i1").getAttribute("aria-pressed")).toBe("true");
    expect(card("i2").getAttribute("aria-pressed")).toBe("false");
    expect(card("i3").getAttribute("aria-pressed")).toBe("true");
    expect(toolbar()!.getAttribute("aria-label")).toBe("2 items selected");

    // Toggling again deselects.
    fireEvent.click(card("i3"));
    expect(toolbar()!.getAttribute("aria-label")).toBe("1 item selected");
  });

  it("a card's long-press Select enters select mode with that card selected", () => {
    render(<IssueFilterViewDetail view={view} />);
    fireEvent.click(card("i2"));
    expect(toolbar()!.getAttribute("aria-label")).toBe("1 item selected");
    expect(card("i2").getAttribute("aria-pressed")).toBe("true");
    // The header trigger mirrors the mode.
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
  });

  it("Mark read / Mark unread send the selected issue ids and exit select mode", () => {
    render(<IssueFilterViewDetail view={view} />);
    fireEvent.click(screen.getByRole("button", { name: "Select" }));
    fireEvent.click(card("i1"));
    fireEvent.click(card("i2"));

    fireEvent.click(
      within(toolbar()!).getAllByRole("button", { name: /Mark read/ })[0]!,
    );
    expect(bulkMutate).toHaveBeenCalledTimes(1);
    expect(bulkMutate.mock.calls[0]![0]).toEqual({
      issue_ids: ["i1", "i2"],
      finished: true,
    });
    // Success handler leaves select mode.
    act(() => {
      (bulkMutate.mock.calls[0]![1] as { onSuccess: () => void }).onSuccess();
    });
    expect(toolbarClosed()).toBe(true);
    expect(card("i1").getAttribute("data-select-active")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "Select" }));
    fireEvent.click(card("i3"));
    fireEvent.click(
      within(toolbar()!).getAllByRole("button", { name: /Mark unread/ })[0]!,
    );
    expect(bulkMutate.mock.calls[1]![0]).toEqual({
      issue_ids: ["i3"],
      finished: false,
    });
  });

  it("Done exits select mode and returns focus to the Select trigger", () => {
    render(<IssueFilterViewDetail view={view} />);
    const select = screen.getByRole("button", { name: "Select" });
    fireEvent.click(select);
    fireEvent.click(card("i1"));
    fireEvent.click(
      screen.getByRole("button", { name: "Done — exit select mode" }),
    );
    expect(toolbarClosed()).toBe(true);
    expect(card("i1").getAttribute("data-select-active")).toBe("false");
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Select" }),
    );
    expect(bulkMutate).not.toHaveBeenCalled();
  });
});
