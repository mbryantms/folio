// @vitest-environment jsdom
/**
 * Metadata apply, batches, accept and bulk edit are admin-only writes
 * (audit 2026-11 E16: the server's routes take `RequireAdmin`). Readers
 * keep search and candidates, so the match dialog still renders its
 * candidates and previews for them, but with no Apply controls; the series
 * Issues tab's selection toolbar offers them no Editing group (Edit
 * metadata… / Fetch metadata / Edit archives…).
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  CandidateView,
  DiffResp,
  MatchOutcomeView,
} from "@/lib/api/types";

let role: "admin" | "user" = "admin";

vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  useSeriesIssuesInfinite: () => ({
    data: {
      pages: [
        {
          items: [
            {
              id: "i1",
              slug: "1",
              series_id: "s1",
              series_slug: "saga",
              title: null,
              number: "1",
              sort_number: 1,
              page_count: 20,
              state: "active",
              cover_url: null,
              read_progress: null,
            },
          ],
          total: 1,
        },
      ],
    },
    isLoading: false,
    isError: false,
    error: null,
    hasNextPage: false,
    isFetchingNextPage: false,
    fetchNextPage: vi.fn(),
  }),
  useMe: () => ({ data: { role } }),
}));
vi.mock("@/lib/api/mutations", async (importOriginal) => {
  const mutation = () => ({ mutate: vi.fn(), isPending: false });
  return {
    ...(await importOriginal<Record<string, unknown>>()),
    useBulkMarkProgress: mutation,
    useBulkMarkSeriesMatchingProgress: mutation,
    useCreateSeriesSelectionBatch: mutation,
    useSetSeriesCover: mutation,
  };
});
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), refresh: vi.fn() }),
}));
vi.mock("next/dynamic", () => ({ default: () => () => null }));
vi.mock("sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
vi.mock("@/components/collections/BulkAddToCollectionDialog", () => ({
  BulkAddToCollectionDialog: () => null,
}));
vi.mock("@/components/library/IssueCard", () => ({
  IssueCard: () => null,
  IssueCardSkeleton: () => null,
}));

import { IssuesPanel } from "@/app/[locale]/(library)/series/[slug]/IssuesPanel";
import { MatchOutcomeBanner } from "@/components/library/MetadataMatchCandidates";
import { MetadataPreviewPane } from "@/components/library/MetadataPreviewPane";

beforeAll(() => {
  // The load-more sentinel observes itself on mount; jsdom has no IO.
  class IOStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  vi.stubGlobal("IntersectionObserver", IOStub);
});

beforeEach(() => {
  role = "admin";
});

function enterSelectMode(container: HTMLElement) {
  const bar = container.querySelector<HTMLElement>("[data-phone-controls]")!;
  fireEvent.click(within(bar).getByRole("button", { name: "View options" }));
  const sheet = screen.getByRole("dialog", { name: "View options" });
  fireEvent.click(within(sheet).getByRole("button", { name: "Select" }));
}

describe("series Issues tab selection toolbar", () => {
  it("offers an admin the Editing group", () => {
    const { container } = render(
      <IssuesPanel seriesSlug="saga" issueCount={1} />,
    );
    enterSelectMode(container);
    expect(screen.getByRole("button", { name: /Editing/ })).toBeTruthy();
  });

  it("gives a reader no Editing group", () => {
    role = "user";
    const { container } = render(
      <IssuesPanel seriesSlug="saga" issueCount={1} />,
    );
    enterSelectMode(container);
    expect(screen.getByRole("toolbar")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Editing/ })).toBeNull();
    expect(screen.queryByText("Edit metadata…")).toBeNull();
  });
});

const strong = {
  kind: "single_good",
  matched_via_alternate: false,
  top_hamming: 2,
} as unknown as MatchOutcomeView;
const top = {
  source: "comicvine",
  external_id: "123",
  bucket: "high",
  score: 95,
  score_breakdown: {},
  candidate: { name: "Saga", year: 2012 },
} as unknown as CandidateView;

describe("<MatchOutcomeBanner> strong match", () => {
  it("gives an admin the one-click Apply", () => {
    render(
      <MatchOutcomeBanner
        outcome={strong}
        topCandidate={top}
        isApplying={false}
        onOneClickApply={() => {}}
        onShowDetails={() => {}}
      />,
    );
    expect(screen.getByRole("button", { name: "Apply" })).toBeTruthy();
  });

  it("shows a reader the match and Show details, without Apply", () => {
    render(
      <MatchOutcomeBanner
        outcome={strong}
        topCandidate={top}
        isApplying={false}
        onOneClickApply={() => {}}
        onShowDetails={() => {}}
        canApply={false}
      />,
    );
    expect(screen.getByText(/Strong match: Saga/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Show details" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
    expect(screen.getByText(/Only an admin can apply it/)).toBeTruthy();
  });
});

const diff: DiffResp = {
  run_id: "00000000-0000-0000-0000-000000000000",
  ordinal: 0,
  scope: "series",
  source: "comicvine",
  source_external_id: "12345",
  rows: [
    {
      field: "summary",
      label: "Summary",
      current_value: null,
      proposed_value: "A space opera.",
      decision: "would_fill",
      current_set_by: null,
      current_set_at: null,
    },
  ],
  external_id_conflicts: [],
  external_ids_new: [],
  changes_count: 1,
} as unknown as DiffResp;

function renderPane(canApply?: boolean) {
  return render(
    <MetadataPreviewPane
      data={diff}
      isLoading={false}
      errorMessage={null}
      selectedFields={new Set(["summary"])}
      overrideExternalIdSources={new Set()}
      onChangeSelected={() => {}}
      onChangeOverrideSources={() => {}}
      onBack={() => {}}
      onApply={() => {}}
      isApplying={false}
      canOverride={canApply ?? true}
      canApply={canApply}
    />,
  );
}

describe("<MetadataPreviewPane> apply button", () => {
  it("lets an admin apply the selected changes", () => {
    renderPane();
    expect(screen.getByRole("button", { name: "Apply 1 change" })).toBeTruthy();
  });

  it("is read-only for a reader", () => {
    renderPane(false);
    expect(screen.getByText("A space opera.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Apply/ })).toBeNull();
    expect(screen.getByText("Only an admin can apply metadata.")).toBeTruthy();
  });
});
