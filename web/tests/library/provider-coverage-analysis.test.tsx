// @vitest-environment jsdom
/**
 * <ProviderCoverageAnalysis> — the provider-coverage grid.
 *
 * Pure helpers (run labels, row collapsing, per-provider grouping) plus the
 * responsive layout: at ≥ 640 px the local-issue × provider grid renders;
 * below it the grid is dropped and each provider's grouped list carries
 * the picture (no sideways scroll at 390 px).
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ProviderCoverageAnalysis,
  gridRows,
  runsLabel,
  seriesGroups,
} from "@/components/library/ProviderCoverageAnalysis";
import { promptHeadline } from "@/components/library/CoverageAfterMatchPrompt";
import { acceptSummary } from "@/lib/api/mutations";
import { analysisAttention, staleRemovedSummary } from "@/lib/coverage-ranges";
import type {
  AcceptOutcome,
  CoverageAnalysisResp,
  CoverageLocalIssue,
  ProviderCoverageView,
} from "@/lib/api/types";

const NUMBERS = ["1", "2", "3", "500", "501", "900"];
const local: CoverageLocalIssue[] = NUMBERS.map((n) => ({
  number: n,
  year: null,
  month: null,
  special: false,
}));

function provider(
  source: string,
  label: string,
  main: string,
  alt: string,
  overrides: Partial<ProviderCoverageView> = {},
): ProviderCoverageView {
  const seriesFor = (n: string) =>
    n === "900" ? null : Number(n) >= 500 ? alt : main;
  return {
    source,
    source_label: label,
    status: "analyzed",
    confidence: "high",
    confidence_reasons: ["main series matches the name and start year"],
    requests: 3,
    request_budget: 40,
    error: null,
    main_series_id: main,
    current_series_id: null,
    current_series_set_by: null,
    candidates: [
      {
        provider_series_id: main,
        name: "Daredevil",
        year: 1998,
        publisher: "Marvel",
        url: `https://example.test/${main}`,
        origin: "search",
        strict: true,
        listed_count: 8,
        local_matches: 3,
        assigned: 3,
        partial: false,
      },
      {
        provider_series_id: alt,
        name: "Daredevil",
        year: 1964,
        publisher: "Marvel",
        url: null,
        origin: "search",
        strict: false,
        listed_count: 18,
        local_matches: 2,
        assigned: 2,
        partial: false,
      },
    ],
    cells: NUMBERS.map((n) => ({
      number: n,
      provider_series_id: seriesFor(n),
      provider_issue_id: seriesFor(n) ? `${source}-${n}` : null,
      date_match: seriesFor(n) ? "confirmed" : null,
    })),
    proposed_ranges: [
      {
        provider_series_id: alt,
        provider_series_name: "Daredevil",
        declared_year: 1964,
        low: "500",
        high: "501",
        issue_count: 2,
        status: "new",
        note: null,
      },
    ],
    uncovered: ["900"],
    unranged_specials: [],
    stale_ranges: [],
    conflicts: [],
    has_changes: true,
    auto_acceptable: true,
    ...overrides,
  };
}

function analysis(providers: ProviderCoverageView[]): CoverageAnalysisResp {
  return {
    job_id: "j",
    series_id: "s",
    state: "done",
    auto_accept: false,
    requested_at: "2026-10-03T00:00:00Z",
    started_at: "2026-10-03T00:00:01Z",
    finished_at: "2026-10-03T00:00:09Z",
    error: null,
    local_issues: local,
    providers,
    auto_accepted: [],
    trigger: "analyze",
  };
}

function setViewport(wide: boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: wide && query === "(min-width: 640px)",
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }));
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("coverage helpers", () => {
  it("labels runs of consecutive local issues", () => {
    expect(runsLabel([0, 1, 2], local)).toBe("#1–3");
    expect(runsLabel([4, 0, 3, 1], local)).toBe("#1–2, #500–501");
    expect(runsLabel([5], local)).toBe("#900");
  });

  it("collapses rows every provider files the same way", () => {
    const rows = gridRows(local, [
      provider("comicvine", "ComicVine", "6458", "2190"),
      provider("metron", "Metron", "200", "100"),
    ]);
    expect(rows.map((r) => [r.first, r.last])).toEqual([
      [0, 2],
      [3, 4],
      [5, 5],
    ]);
    expect(rows[2]!.series).toEqual([null, null]);
  });

  it("splits a row when one provider disagrees", () => {
    const m = provider("metron", "Metron", "200", "100");
    m.cells[1] = { ...m.cells[1]!, provider_series_id: "100" };
    const rows = gridRows(local, [
      provider("comicvine", "ComicVine", "6458", "2190"),
      m,
    ]);
    expect(rows.map((r) => [r.first, r.last])).toEqual([
      [0, 0],
      [1, 1],
      [2, 2],
      [3, 4],
      [5, 5],
    ]);
  });

  it("groups a provider's issues by series, main first, uncovered last", () => {
    const p = provider("gcd", "GCD", "4000", "3000");
    // Put the range series first in cell order to prove main sorts first.
    p.main_series_id = "3000";
    const groups = seriesGroups(p);
    expect(groups.map((g) => [g.seriesId, g.role, g.indices])).toEqual([
      ["3000", "main", [3, 4]],
      ["4000", "range", [0, 1, 2]],
      [null, "uncovered", [5]],
    ]);
  });
});

describe("<ProviderCoverageAnalysis>", () => {
  const data = analysis([
    provider("comicvine", "ComicVine", "6458", "2190"),
    provider("metron", "Metron", "200", "100", {
      confidence: "medium",
      has_changes: false,
    }),
    {
      ...provider("gcd", "Grand Comics Database", "4000", "3000"),
      status: "not_configured",
      candidates: [],
      cells: [],
    },
  ]);

  it("renders the issue × provider grid on wide screens", async () => {
    setViewport(true);
    render(
      <ProviderCoverageAnalysis
        data={data}
        acceptingSource={null}
        onAccept={() => {}}
        onRemoveStale={() => {}}
      />,
    );
    const grid = await screen.findByTestId("coverage-grid");
    // Header: Issues + the two analysed providers (GCD isn't configured).
    const headers = Array.from(grid.querySelectorAll("th")).map(
      (th) => th.textContent,
    );
    expect(headers).toEqual(["Issues", "ComicVine", "Metron"]);
    const firstCells = Array.from(grid.querySelectorAll("tbody tr")).map(
      (tr) => tr.querySelector("td")?.textContent,
    );
    expect(firstCells).toEqual(["#1–3", "#500–501", "#900"]);
  });

  it("collapses to grouped lists on narrow screens", async () => {
    setViewport(false);
    render(
      <ProviderCoverageAnalysis
        data={data}
        acceptingSource={null}
        onAccept={() => {}}
        onRemoveStale={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.getByTestId("coverage-list-comicvine")).toBeTruthy(),
    );
    expect(screen.queryByTestId("coverage-grid")).toBeNull();
    const list = screen.getByTestId("coverage-list-comicvine");
    expect(list.textContent).toContain("Daredevil (1998)");
    expect(list.textContent).toContain("#1–3");
    expect(list.textContent).toContain("#500–501");
    expect(
      screen.getByTestId("coverage-uncovered-comicvine").textContent,
    ).toContain("No ComicVine series has #900");
    expect(screen.getByTestId("coverage-provider-gcd").textContent).toContain(
      "Set up Grand Comics Database",
    );
  });

  it("accepts the proposal and disables providers already up to date", async () => {
    setViewport(false);
    const onAccept = vi.fn();
    render(
      <ProviderCoverageAnalysis
        data={data}
        acceptingSource={null}
        onAccept={onAccept}
        onRemoveStale={() => {}}
      />,
    );
    const cv = await screen.findByTestId("coverage-provider-comicvine");
    fireEvent.click(
      Array.from(cv.querySelectorAll("button")).find(
        (b) => b.textContent === "Accept",
      )!,
    );
    expect(onAccept).toHaveBeenCalledWith("comicvine", null);
    const metron = screen.getByTestId("coverage-provider-metron");
    const upToDate = Array.from(metron.querySelectorAll("button")).find(
      (b) => b.textContent === "Up to date",
    )!;
    expect(upToDate.hasAttribute("disabled")).toBe(true);
    expect(metron.textContent).toContain("Medium confidence");
  });
});

/** The owner's FF (2001) case: GCD #42–70 → Fantastic Four (1961). */
function gcdCleanup(): AcceptOutcome {
  return {
    source: "gcd",
    main_series_id: "11218",
    main_written: false,
    main_note: null,
    ranges_created: [],
    ranges_skipped: [],
    stale_ranges: [],
    stale_ranges_removed: [
      {
        id: "r1",
        provider_series_id: "1482",
        provider_series_name: "Fantastic Four",
        declared_year: 1961,
        range_low: "42",
        range_high: "70",
        set_by: "cross_reference",
        reason: "the proposal puts these issues in another series",
      },
    ],
  };
}

describe("stale ranges an accept removed", () => {
  it("summarises them with provider, numbers and target", () => {
    expect(staleRemovedSummary(gcdCleanup())).toBe(
      "Removed 1 stale range: GCD #42–70 → Fantastic Four (1961)",
    );
    expect(
      staleRemovedSummary({ ...gcdCleanup(), stale_ranges_removed: [] }),
    ).toBeNull();
    // The accept toast leads with it when nothing else changed …
    expect(acceptSummary(gcdCleanup())).toBe(
      "Removed 1 stale range: GCD #42–70 → Fantastic Four (1961)",
    );
    // … and appends it otherwise.
    expect(acceptSummary({ ...gcdCleanup(), main_written: true })).toBe(
      "Coverage accepted: series linked. Removed 1 stale range: GCD #42–70 → Fantastic Four (1961)",
    );
  });

  it("asks to accept when only stale ranges would change", () => {
    const p = provider("gcd", "GCD", "11218", "62349", {
      current_series_id: "11218",
      proposed_ranges: [],
      stale_ranges: gcdCleanup().stale_ranges_removed!,
      has_changes: true,
    });
    expect(promptHeadline(p, 173, false)).toBe(
      "GCD: 1 stale range no longer fits — accept to remove it?",
    );
  });

  it("lists removals from accepts and automatic accepts above the providers", async () => {
    setViewport(false);
    const data = {
      ...analysis([provider("gcd", "GCD", "11218", "62349")]),
      auto_accepted: [gcdCleanup()],
    };
    render(
      <ProviderCoverageAnalysis
        data={data}
        acceptingSource={null}
        onAccept={() => {}}
        onRemoveStale={() => {}}
        removedNotes={["Removed 1 stale range: Metron #1–5 → Daredevil (1998)"]}
      />,
    );
    const list = await screen.findByTestId("coverage-stale-removed");
    await waitFor(() =>
      expect(list.textContent).toContain(
        "Removed 1 stale range: GCD #42–70 → Fantastic Four (1961)",
      ),
    );
    expect(list.textContent).toContain("Metron #1–5 → Daredevil (1998)");
    expect(list.getAttribute("role")).toBe("status");
  });
});

describe("analysisAttention (collapsed-by-default rule)", () => {
  const upToDate = (source: string, label: string) =>
    provider(source, label, "100", "200", {
      has_changes: false,
      proposed_ranges: [],
      uncovered: [],
    });

  it("is quiet once every analysed provider is up to date", () => {
    const a = analysisAttention(
      analysis([upToDate("metron", "Metron"), upToDate("gcd", "GCD")]),
    );
    expect(a.needsAttention).toBe(false);
    expect(a.summary).toMatch(/^all providers up to date · finished /);
  });

  it("opens for a pending proposal, and counts the providers", () => {
    const a = analysisAttention(
      analysis([
        provider("metron", "Metron", "100", "200"),
        upToDate("gcd", "GCD"),
      ]),
    );
    expect(a.needsAttention).toBe(true);
    expect(a.summary).toMatch(/^1 provider needs review/);
    const b = analysisAttention(
      analysis([
        provider("metron", "Metron", "100", "200"),
        provider("gcd", "GCD", "100", "200"),
      ]),
    );
    expect(b.summary).toMatch(/^2 providers need review/);
  });

  it("opens for stale mappings and conflicts even with no proposal", () => {
    const stale = upToDate("metron", "Metron");
    stale.stale_ranges = [
      {
        id: "r1",
        provider_series_id: "300",
        provider_series_name: "Old",
        range_low: "1",
        range_high: "5",
        reason: "no longer listed",
      } as (typeof stale.stale_ranges)[number],
    ];
    expect(analysisAttention(analysis([stale])).needsAttention).toBe(true);
    const conflict = upToDate("gcd", "GCD");
    conflict.conflicts = ["#7 is pinned by a user"];
    expect(analysisAttention(analysis([conflict])).needsAttention).toBe(true);
  });

  it("treats errored / rate-limited / partial providers as needing a look", () => {
    for (const status of [
      "error",
      "rate_limited",
      "not_listable",
      "partial",
    ] as const) {
      const p = upToDate("metron", "Metron");
      p.status = status;
      expect(analysisAttention(analysis([p])).needsAttention, status).toBe(
        true,
      );
    }
  });

  it("ignores providers that are merely not configured or had no candidates", () => {
    const off = upToDate("comicvine", "ComicVine");
    off.status = "not_configured";
    const none = upToDate("gcd", "GCD");
    none.status = "no_candidates";
    const a = analysisAttention(
      analysis([upToDate("metron", "Metron"), off, none]),
    );
    expect(a.needsAttention).toBe(false);
    const b = analysisAttention(analysis([off, none]));
    expect(b.needsAttention).toBe(false);
    expect(b.summary).toMatch(/^no provider could be analysed/);
  });

  it("stays open while a run is in progress or after a failure", () => {
    const running = {
      ...analysis([]),
      state: "running" as const,
      finished_at: null,
    };
    expect(analysisAttention(running)).toEqual({
      needsAttention: true,
      summary: "Running in the background…",
    });
    const failed = { ...analysis([]), state: "failed" as const, error: "boom" };
    expect(analysisAttention(failed).needsAttention).toBe(true);
  });
});
