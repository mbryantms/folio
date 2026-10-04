// @vitest-environment jsdom
/**
 * "Match this series…" coverage hints (coverage tie-ins PR 2).
 *
 * - `formatCoverageHint`: the one-line summary ("Covers 160 of your 173
 *   issues · #600–611 aren't in this series").
 * - The real `<MetadataMatchDialog>` (series scope) over a faked network:
 *   once the run is completed it asks for the top three candidates' hints
 *   in one request, shows them under each card without reordering, and
 *   asks for a lower candidate only when its "Check coverage" is clicked.
 * - `<CoverageAfterMatchPrompt>`: the post-match result on the Details tab
 *   ("This folder spans 2 Metron series — accept the ranges?").
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type {
  CandidatesResp,
  CoverageAnalysisResp,
  CoverageHintView,
  ProviderCoverageView,
  SeriesCoverageHint,
} from "@/lib/api/types";

const net = vi.hoisted(() => ({
  calls: [] as string[],
  hintBodies: new Map<string, unknown>(),
}));

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

const CANDIDATES = [
  ["metron", "1711", "Fantastic Four", 1998, "medium", 72.5],
  ["comicvine", "6211", "Fantastic Four", 1998, "medium", 70],
  ["metron", "1713", "Fantastic Four", 2012, "low", 41],
  ["gcd", "11218", "Fantastic Four", 1998, "low", 40],
] as const;

function candidatesResp(): CandidatesResp {
  return {
    run_id: "run-1",
    status: "completed",
    providers: ["metron", "comicvine", "gcd"],
    started_at: "2026-10-03T00:00:00Z",
    finished_at: "2026-10-03T00:00:05Z",
    items_total: 1,
    items_matched_high: 0,
    items_matched_medium: 1,
    items_matched_low: 0,
    error_summary: null,
    candidates: CANDIDATES.map(([source, id, name, year, bucket, score]) => ({
      source,
      external_id: id,
      bucket,
      score,
      score_breakdown: {},
      candidate: { name, year, publisher: "Marvel" },
    })),
  };
}

function hint(
  ordinal: number,
  over: Partial<SeriesCoverageHint> = {},
): CoverageHintView {
  const [source, id] = CANDIDATES[ordinal]!;
  return {
    ordinal,
    source,
    external_id: id,
    status: "computed",
    reason: null,
    local_total: 173,
    covered: 173,
    date_confirmed: 173,
    date_conflicts: 0,
    missing_count: 0,
    missing_runs: [],
    listed_count: 173,
    partial: false,
    requests: 0,
    ...over,
  };
}

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (input: string) => {
    const url = String(input);
    net.calls.push(url);
    if (url.includes("/auth/me")) {
      return json({ id: "u1", role: "admin", email: "a@b.c" });
    }
    if (url.includes("/metadata/coverage-hints")) {
      const ordinals = new URL(url, "http://x").searchParams.get("ordinals")!;
      return json(net.hintBodies.get(ordinals) ?? { hints: [] });
    }
    if (url.includes("/metadata/candidates")) {
      return json(candidatesResp());
    }
    return json({});
  }),
  getCsrfToken: () => "csrf",
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh: () => undefined }),
}));

vi.mock("@/lib/api/scan-events", () => ({
  useScanEvents: () => ({ status: "open" as const, events: [] }),
}));

import { CoverageAfterMatchPrompt } from "@/components/library/CoverageAfterMatchPrompt";
import { MetadataMatchDialog } from "@/components/library/MetadataMatchDialog";
import { formatCoverageHint } from "@/lib/metadata/coverage-hint";

describe("formatCoverageHint", () => {
  it("summarises partial coverage with the missing runs", () => {
    expect(
      formatCoverageHint(
        hint(0, {
          covered: 160,
          missing_count: 13,
          missing_runs: ["#600–611"],
        }),
      ),
    ).toBe("Covers 160 of your 173 issues · #600–611 aren't in this series");
  });

  it("says when every issue is covered", () => {
    expect(formatCoverageHint(hint(1))).toBe("Covers all 173 of your issues");
  });

  it("flags date conflicts, a single missing issue and partial lists", () => {
    expect(
      formatCoverageHint(
        hint(2, {
          covered: 10,
          local_total: 12,
          missing_count: 1,
          missing_runs: ["#3"],
          date_conflicts: 1,
          partial: true,
        }),
      ),
    ).toBe(
      "Covers 10 of your 12 issues · #3 isn't in this series · 1 cover date disagree (issue list only partly read)",
    );
  });

  it("explains a skipped hint", () => {
    expect(
      formatCoverageHint(
        hint(3, { status: "not_computed", reason: "budget", covered: 0 }),
      ),
    ).toMatch(/^Coverage not computed — .*budget/);
  });
});

function renderForm() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MetadataMatchDialog
        open={true}
        onOpenChange={() => undefined}
        scope={{
          kind: "series",
          seriesSlug: "fantastic-four",
          libraryId: "lib-1",
        }}
      />
    </QueryClientProvider>,
  );
}

describe("Match this series… coverage hints", () => {
  it("hints the top three in one request and the rest on demand", async () => {
    net.calls = [];
    net.hintBodies = new Map<string, unknown>([
      [
        "0,1,2",
        {
          max_per_request: 3,
          hints: [
            hint(0, {
              covered: 160,
              missing_count: 13,
              missing_runs: ["#600–611"],
              requests: 2,
            }),
            hint(1),
            hint(2, { covered: 13, missing_count: 160 }),
          ],
        },
      ],
      [
        "3",
        {
          max_per_request: 3,
          hints: [hint(3, { status: "not_computed", reason: "budget" })],
        },
      ],
    ]);
    renderForm();

    await waitFor(() =>
      expect(
        screen.getByText(
          "Covers 160 of your 173 issues · #600–611 aren't in this series",
        ),
      ).toBeTruthy(),
    );
    expect(screen.getByText("Covers all 173 of your issues")).toBeTruthy();

    const hintCalls = () =>
      net.calls.filter((u) => u.includes("/metadata/coverage-hints"));
    expect(hintCalls()).toHaveLength(1);
    expect(hintCalls()[0]).toContain("run_id=run-1");
    expect(hintCalls()[0]).toContain("ordinals=0,1,2");

    // Order unchanged: the hints don't rerank.
    const hints = screen.getAllByTestId("coverage-hint");
    expect(hints).toHaveLength(3);
    expect(hints[0]!.textContent).toContain("Covers 160");
    expect(hints[1]!.textContent).toContain("Covers all 173");
    expect(hints[2]!.textContent).toContain("Covers 13");

    // The fourth candidate (GCD) waits for a click.
    const check = screen.getByRole("button", { name: "Check coverage" });
    fireEvent.click(check);
    await waitFor(() =>
      expect(screen.getByText(/Coverage not computed/)).toBeTruthy(),
    );
    expect(hintCalls()).toHaveLength(2);
    expect(hintCalls()[1]).toContain("ordinals=3");
    expect(screen.queryByRole("button", { name: "Check coverage" })).toBeNull();
  });
});

function metronView(
  over: Partial<ProviderCoverageView> = {},
): ProviderCoverageView {
  const cells = [
    ...Array.from({ length: 160 }, (_, i) => ({
      number: String(i),
      provider_series_id: "1711",
      provider_issue_id: null,
      date_match: "confirmed" as const,
    })),
    ...Array.from({ length: 13 }, (_, i) => ({
      number: String(600 + i),
      provider_series_id: "1713",
      provider_issue_id: null,
      date_match: "confirmed" as const,
    })),
  ];
  return {
    source: "metron",
    source_label: "Metron",
    status: "analyzed",
    confidence: "high",
    confidence_reasons: [],
    requests: 2,
    request_budget: 30,
    error: null,
    main_series_id: "1711",
    seeded_series_id: "1711",
    current_series_id: "1711",
    current_series_set_by: "metron",
    candidates: [],
    cells,
    proposed_ranges: [
      {
        provider_series_id: "1713",
        provider_series_name: "Fantastic Four",
        declared_year: 2012,
        low: "600",
        high: "611",
        issue_count: 13,
        status: "new",
        note: null,
      },
    ],
    uncovered: [],
    unranged_specials: [],
    stale_ranges: [],
    conflicts: [],
    has_changes: true,
    auto_acceptable: true,
    ...over,
  };
}

function analysis(
  providers: ProviderCoverageView[],
  over: Partial<CoverageAnalysisResp> = {},
): CoverageAnalysisResp {
  return {
    job_id: "j",
    series_id: "s",
    state: "done",
    auto_accept: false,
    requested_at: "2026-10-03T00:00:00Z",
    started_at: "2026-10-03T00:00:01Z",
    finished_at: "2026-10-03T00:00:09Z",
    error: null,
    local_issues: Array.from({ length: 173 }, (_, i) => ({
      number: String(i),
      year: null,
      month: null,
      special: false,
    })),
    providers,
    auto_accepted: [],
    trigger: "series_match",
    ...over,
  };
}

describe("<CoverageAfterMatchPrompt>", () => {
  it("asks to accept the ranges of a folder spanning two series", () => {
    const onAccept = vi.fn();
    render(
      <CoverageAfterMatchPrompt
        data={analysis([metronView()])}
        acceptingSource={null}
        onAccept={onAccept}
      />,
    );
    expect(screen.getByText("After your series match")).toBeTruthy();
    expect(
      screen.getByText("This folder spans 2 Metron series — accept the range?"),
    ).toBeTruthy();
    expect(screen.getByText("#600–611 → Fantastic Four (2012)")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Accept" }));
    expect(onAccept).toHaveBeenCalledWith("metron", null);
  });

  it("says when the match covers everything, and stays hidden for a manual analysis", () => {
    const covered = metronView({
      proposed_ranges: [],
      has_changes: false,
      cells: metronView().cells.map((c) => ({
        ...c,
        provider_series_id: "1711",
      })),
    });
    const { rerender } = render(
      <CoverageAfterMatchPrompt
        data={analysis([covered])}
        acceptingSource={null}
        onAccept={() => undefined}
      />,
    );
    expect(
      screen.getByText(
        "Metron: your match covers all 173 issues — nothing left to accept.",
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Accept" })).toBeNull();

    rerender(
      <CoverageAfterMatchPrompt
        data={analysis([covered], { trigger: "analyze" })}
        acceptingSource={null}
        onAccept={() => undefined}
      />,
    );
    expect(screen.queryByTestId("coverage-after-match")).toBeNull();
  });

  it("reports an automatic accept instead of offering one", () => {
    render(
      <CoverageAfterMatchPrompt
        data={analysis([metronView()], {
          trigger: "bulk_series_match",
          auto_accepted: [
            {
              source: "metron",
              main_series_id: "1711",
              main_written: false,
              main_note: null,
              ranges_created: [],
              ranges_skipped: [],
              stale_ranges: [],
            },
          ],
        })}
        acceptingSource={null}
        onAccept={() => undefined}
      />,
    );
    expect(screen.getByText("After the bulk series match")).toBeTruthy();
    expect(
      screen.getByText("Metron: accepted automatically (high confidence)."),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Accept" })).toBeNull();
  });
});
