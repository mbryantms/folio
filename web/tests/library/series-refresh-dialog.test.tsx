// @vitest-environment jsdom
/**
 * Guided "Refresh this series…" (coverage tie-ins PR 3).
 *
 * The real `<SeriesRefreshDialog>` / `<SeriesRefreshFlow>` over a faked
 * server whose state the tests move along (`srv`): the refresh status
 * (`resume_step`, links, coverage job, batch, fetch estimate), the
 * coverage analysis, the batch and the match run. Every request is
 * recorded so the tests assert what the flow sent — and what it didn't.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as React from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  BatchStatusResp,
  CoverageAnalysisResp,
  ProviderCoverageView,
  SeriesRefreshStatusResp,
} from "@/lib/api/types";

type Call = { method: string; url: string; body: unknown };

const srv = vi.hoisted(() => ({
  batchItems: 173,
  batchEligible: 173,
  calls: [] as Call[],
  status: null as unknown,
  analysis: null as unknown,
  batch: null as unknown,
  onApply: null as null | (() => void),
  onAccept: null as null | ((source: string) => void),
}));

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async (input: string, init?: RequestInit) => {
    const url = String(input);
    const method = (init?.method ?? "GET").toUpperCase();
    const body =
      typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    srv.calls.push({ method, url, body });
    if (url.includes("/auth/me")) {
      return json({ id: "u1", role: "admin", email: "a@b.c" });
    }
    if (url.includes("/metadata/refresh-status")) return json(srv.status);
    if (url.includes("/provider-coverage/analysis")) {
      return srv.analysis
        ? json(srv.analysis)
        : json({ error: { code: "x", message: "none" } }, 404);
    }
    if (url.includes("/provider-coverage/accept")) {
      srv.onAccept?.((body as { source: string }).source);
      return json({
        source: (body as { source: string }).source,
        main_series_id: "1711",
        main_written: false,
        main_note: null,
        ranges_created: [],
        ranges_skipped: [],
        stale_ranges: [],
      });
    }
    if (url.includes("/provider-coverage/analyze")) {
      return json({ job_id: "j-new", state: "queued", queued: true }, 202);
    }
    if (url.includes("/metadata/coverage-hints")) {
      return json({ max_per_request: 3, hints: [] });
    }
    if (url.includes("/metadata/lookup")) {
      return json({ run_id: "run-1", source: "metron", external_id: "1711" });
    }
    if (url.includes("/metadata/candidates")) return json(candidates());
    if (url.includes("/metadata/apply") && url.includes("/series/")) {
      srv.onApply?.();
      return json({ run_id: "run-1", ordinal: 0 }, 202);
    }
    if (url.includes("/metadata/batch/b1/apply")) {
      return json({ enqueued: 3, skipped: 0, remainder: 0 });
    }
    if (url.includes("/metadata/batch/b1")) return json(srv.batch);
    if (url.includes("/metadata/batch")) {
      return json(
        {
          batch_id: "b1",
          items_total: srv.batchItems,
          jobs_enqueued: srv.batchItems,
          jobs_coalesced: 0,
          jobs_failed: 0,
          eligible: srv.batchEligible,
          recently_fetched: 0,
          remainder: srv.batchEligible - srv.batchItems,
        },
        202,
      );
    }
    return json({});
  }),
  getCsrfToken: () => "csrf",
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh: () => undefined, push: () => undefined }),
}));

vi.mock("next/link", () => ({
  default: ({
    href,
    children,
    ...rest
  }: React.AnchorHTMLAttributes<HTMLAnchorElement> & { href: string }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

// The apply waits for the library's completion event; deliver one as soon
// as the dialog subscribes (it only subscribes while waiting).
vi.mock("@/lib/api/scan-events", () => ({
  useScanEvents: ({ libraryId }: { libraryId?: string }) => ({
    status: "open" as const,
    events: libraryId ? [{ type: "metadata.applied" }] : [],
  }),
}));

import {
  SeriesRefreshDialog,
  SeriesRefreshFlow,
  estimateLine,
} from "@/components/library/SeriesRefreshDialog";

// ───────── fixtures ─────────

const T0 = "2026-10-03T10:00:00Z";
const T1 = "2026-10-03T10:05:00Z";
const T2 = "2026-10-03T10:06:00Z";

function status(
  over: Partial<SeriesRefreshStatusResp> = {},
): SeriesRefreshStatusResp {
  return {
    series_id: "s1",
    resume_step: "coverage",
    series_match: { links: [], latest_run: null, applied_at: null },
    coverage: null,
    coverage_after_series_apply: "manual_only",
    batch: null,
    fetch_estimate: [
      {
        scope: "all",
        issues: 173,
        eligible: 173,
        recently_fetched: 0,
        remainder: 0,
        providers: [
          { source: "comicvine", direct: 170, search: 3 },
          { source: "metron", direct: 173, search: 0 },
          { source: "gcd", direct: 0, search: 173 },
        ],
      },
      {
        scope: "incomplete",
        issues: 12,
        eligible: 12,
        recently_fetched: 0,
        remainder: 0,
        providers: [
          { source: "comicvine", direct: 12, search: 0 },
          { source: "metron", direct: 12, search: 0 },
          { source: "gcd", direct: 0, search: 12 },
        ],
      },
    ],
    ...over,
  };
}

function candidates() {
  return {
    run_id: "run-1",
    status: "completed",
    providers: ["metron"],
    started_at: T0,
    finished_at: T0,
    items_total: 1,
    items_matched_high: 1,
    items_matched_medium: 0,
    items_matched_low: 0,
    error_summary: null,
    match_outcome: { kind: "single_good", matched_via_alternate: false },
    candidates: [
      {
        source: "metron",
        external_id: "1711",
        bucket: "high",
        score: 92,
        score_breakdown: {},
        candidate: { name: "Fantastic Four", year: 1998, publisher: "Marvel" },
      },
    ],
  };
}

function provider(
  source: string,
  label: string,
  over: Partial<ProviderCoverageView> = {},
): ProviderCoverageView {
  return {
    source,
    source_label: label,
    status: "analyzed",
    confidence: "high",
    confidence_reasons: [],
    requests: 2,
    request_budget: 30,
    error: null,
    main_series_id: "1711",
    seeded_series_id: null,
    current_series_id: "1711",
    current_series_set_by: null,
    candidates: [],
    cells: [
      {
        number: "1",
        provider_series_id: "1711",
        provider_issue_id: null,
        date_match: "confirmed",
      },
      {
        number: "600",
        provider_series_id: "1713",
        provider_issue_id: null,
        date_match: "confirmed",
      },
    ],
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

function coverage(
  providers: ProviderCoverageView[],
  over: Partial<CoverageAnalysisResp> = {},
): CoverageAnalysisResp {
  return {
    job_id: "j1",
    series_id: "s1",
    state: "done",
    auto_accept: false,
    requested_at: T1,
    started_at: T1,
    finished_at: T1,
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

const coverageJob = (requested_at = T1) => ({
  job_id: "j1",
  state: "done" as const,
  trigger: "series_match" as const,
  requested_at,
  finished_at: requested_at,
  sources: ["metron"],
});

function batch(
  state: "running" | "completed",
  over: Partial<BatchStatusResp> = {},
): BatchStatusResp {
  const child = (i: number, outcome_kind: string) => ({
    run_id: `r${i}`,
    scope: "issue",
    scope_entity_id: `i${i}`,
    label: `Fantastic Four #${i}`,
    status: state === "running" && i > 2 ? "searching" : "completed",
    outcome_kind,
    applied: false,
    series_slug: "fantastic-four",
    issue_slug: `${i}`,
    library_id: "lib-1",
  });
  return {
    batch_id: "b1",
    scope: "series_issues",
    status: state,
    items_total: 173,
    created_at: T2,
    aggregate: {
      searched: state === "running" ? 40 : 173,
      strong: 3,
      needs_review: 2,
      no_match: 1,
      applied: 0,
      awaiting_quota: 0,
      failed: 0,
      in_flight: state === "running" ? 133 : 0,
      partial: 0,
      lookups: [{ source: "comicvine", direct: 170, search: 3, fallbacks: [] }],
    },
    children: [
      child(1, "single_good"),
      child(2, "single_good"),
      child(3, "single_good"),
      child(4, "multi_good"),
      child(5, "single_bad_cover"),
      child(6, "no_match"),
    ],
    budget: [],
    exceeds_budget: false,
    resume_eta: null,
    ...over,
  };
}

// ───────── helpers ─────────

function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

function renderFlow(onClose = () => undefined) {
  return render(
    <QueryClientProvider client={client()}>
      <SeriesRefreshFlow
        seriesSlug="fantastic-four"
        libraryId="lib-1"
        onClose={onClose}
      />
    </QueryClientProvider>,
  );
}

const sent = (method: string, part: string) =>
  srv.calls.filter((c) => c.method === method && c.url.includes(part));

async function heading(text: RegExp) {
  return waitFor(() => screen.getByRole("heading", { name: text }));
}

function stepButton(name: RegExp) {
  return screen.getByRole("button", { name });
}

beforeEach(() => {
  srv.calls = [];
  srv.status = status();
  srv.analysis = null;
  srv.batch = null;
  srv.onApply = null;
  srv.onAccept = null;
  srv.batchItems = 173;
  srv.batchEligible = 173;
});

// ───────── tests ─────────

describe("estimateLine", () => {
  it("prices direct lookups at one request and searches at one to two", () => {
    expect(estimateLine({ source: "comicvine", direct: 170, search: 3 })).toBe(
      "ComicVine: 170 direct · 3 searched (≈ 173–176 requests)",
    );
    expect(estimateLine({ source: "metron", direct: 12, search: 0 })).toBe(
      "Metron: 12 direct · 0 searched (≈ 12 requests)",
    );
  });
});

describe("<SeriesRefreshFlow>", () => {
  it("walks coverage → confirm series → fetch → review", async () => {
    // Coverage first, on the folder's own tags: the analysis proposes a
    // Metron main (1711) plus a range; accepting links the series.
    const main1711 = [
      {
        provider_series_id: "1711",
        name: "Fantastic Four",
        year: 1998,
        publisher: "Marvel",
        url: "https://metron.cloud/series/1711/",
        origin: "search" as const,
        strict: true,
        listed_count: 173,
        local_matches: 160,
        assigned: 160,
        partial: false,
      },
    ];
    srv.analysis = coverage([
      provider("metron", "Metron", {
        current_series_id: null,
        candidates: main1711,
      }),
    ]);
    srv.onAccept = () => {
      srv.analysis = coverage([
        provider("metron", "Metron", {
          has_changes: false,
          proposed_ranges: [],
          candidates: main1711,
        }),
      ]);
      srv.status = status({
        resume_step: "match",
        series_match: {
          links: [{ source: "metron", external_id: "1711", set_by: "metron" }],
          latest_run: null,
          applied_at: null,
        },
        coverage: coverageJob(),
      });
    };
    srv.status = status({ resume_step: "coverage", coverage: coverageJob() });
    renderFlow();

    // 1. Coverage: the proposal, accepted here.
    await heading(/Step 1 of 4: Coverage/);
    await waitFor(
      () =>
        expect(
          screen.getByText(
            "This folder spans 2 Metron series — accept the range?",
          ),
        ).toBeTruthy(),
      { timeout: 5000 },
    );
    expect(screen.getByText("#600–611 → Fantastic Four (2012)")).toBeTruthy();
    expect(screen.getByText(/2 of 30 requests/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Accept Metron" }));
    await waitFor(() => expect(screen.getByText("Done")).toBeTruthy());
    expect(sent("POST", "/provider-coverage/accept")[0]!.body).toEqual({
      source: "metron",
      main_series_id: null,
    });

    // 2. Confirm series: the series coverage found is offered; "Use this
    //    series" fetches that exact page (no search) into the match form.
    fireEvent.click(
      screen.getByRole("button", { name: "Continue to confirm series" }),
    );
    await heading(/Step 2 of 4: Confirm series/);
    expect(document.activeElement).toBe(
      screen.getByRole("heading", { name: /Step 2 of 4/ }),
    );
    await waitFor(() =>
      expect(screen.getByText(/Fantastic Four \(1998\)/)).toBeTruthy(),
    );
    expect(screen.getByText("linked")).toBeTruthy();
    srv.onApply = () => {
      srv.status = status({
        resume_step: "fetch",
        series_match: {
          links: [{ source: "metron", external_id: "1711", set_by: "metron" }],
          latest_run: { run_id: "run-1", status: "completed", started_at: T0 },
          applied_at: T1,
        },
        coverage: coverageJob(),
      });
    };
    fireEvent.click(screen.getByRole("button", { name: "Use this series" }));
    const apply = await waitFor(() =>
      screen.getByRole("button", { name: "Apply" }),
    );
    expect(sent("POST", "/metadata/lookup")).toHaveLength(1);
    expect(sent("POST", "/metadata/lookup")[0]!.body).toEqual({
      url: "https://metron.cloud/series/1711/",
    });
    expect(sent("POST", "/metadata/search")).toHaveLength(0);
    fireEvent.click(apply);

    // 3. Per-issue fetch: "All issues", with the quota estimate.
    await heading(/Step 3 of 4: Per-issue fetch/);
    fireEvent.click(screen.getByRole("radio", { name: /All issues/ }));
    expect(
      screen.getByText(
        "ComicVine: 170 direct · 3 searched (≈ 173–176 requests)",
      ),
    ).toBeTruthy();
    srv.batch = batch("completed");
    fireEvent.click(screen.getByRole("button", { name: "Fetch 173 issues" }));

    // 4. The batch finished → review, with Accept all strong.
    await heading(/Step 4 of 4: Review/);
    const batchPosts = sent("POST", "/series/fantastic-four/metadata/batch");
    expect(batchPosts).toHaveLength(1);
    expect(batchPosts[0]!.url).not.toContain("scope=");
    expect(screen.getByText("ComicVine: 170 direct · 3 searched")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Accept all strong (3)" }),
    );
    await waitFor(() =>
      expect(sent("POST", "/metadata/batch/b1/apply")).toHaveLength(1),
    );
    expect(sent("POST", "/metadata/batch/b1/apply")[0]!.body).toEqual({
      filter: "all_strong",
    });

    // Every step done in the stepper.
    for (const name of [
      /1\. Coverage/,
      /2\. Confirm series/,
      /3\. Per-issue/,
    ]) {
      expect(stepButton(name).textContent).toContain("(done)");
    }
  });

  it("keeps the current match without searching", async () => {
    srv.status = status({
      resume_step: "match",
      series_match: {
        links: [
          { source: "comicvine", external_id: "6211", set_by: "user" },
          { source: "metron", external_id: "1711", set_by: "metron" },
        ],
        latest_run: null,
        applied_at: null,
      },
      coverage: coverageJob(),
    });
    renderFlow();
    await heading(/Step 2 of 4: Confirm series/);
    expect(screen.getByText("#6211")).toBeTruthy();
    // Linked ids are listed without a who-set-it label.
    expect(screen.getByText("#1711")).toBeTruthy();
    expect(screen.queryByText("set by you")).toBeNull();
    expect(screen.queryByText("from a match")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Keep current match" }));

    await heading(/Step 3 of 4: Per-issue fetch/);
    expect(stepButton(/2\. Confirm series/).textContent).toContain("(skipped)");
    // No series search, no candidate probe, no lookup.
    expect(sent("POST", "/metadata/search")).toHaveLength(0);
    expect(sent("GET", "/metadata/candidates")).toHaveLength(0);
    expect(sent("POST", "/metadata/lookup")).toHaveLength(0);
  });

  it("offers the analysis with its budget, or skipping, when no job exists", async () => {
    srv.status = status({ resume_step: "coverage" });
    renderFlow();
    await heading(/Step 1 of 4: Coverage/);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Analyze coverage" }),
      ).toBeTruthy(),
    );
    expect(screen.getByText(/40 ComicVine, 30 Metron and 30 GCD/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Skip coverage" }));
    await heading(/Step 2 of 4: Confirm series/);
    expect(stepButton(/1\. Coverage/).textContent).toContain("(skipped)");
    expect(sent("POST", "/provider-coverage/analyze")).toHaveLength(0);
    // Nothing linked and nothing from coverage → straight to the search form.
    await waitFor(() => screen.getByRole("button", { name: "Apply" }));
  });

  it("accepts one provider's coverage and skips another", async () => {
    srv.status = status({
      resume_step: "coverage",
      coverage: { ...coverageJob(), sources: ["comicvine", "metron"] },
    });
    srv.analysis = coverage([
      provider("comicvine", "ComicVine"),
      provider("metron", "Metron"),
    ]);
    renderFlow();
    await heading(/Step 1 of 4: Coverage/);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Accept Metron" }),
      ).toBeTruthy(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Skip ComicVine" }));
    expect(screen.getByText("Skipped")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Accept ComicVine" }),
    ).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Accept Metron" }));
    await waitFor(() =>
      expect(sent("POST", "/provider-coverage/accept")).toHaveLength(1),
    );
    expect(sent("POST", "/provider-coverage/accept")[0]!.body).toEqual({
      source: "metron",
      main_series_id: null,
    });
  });

  it("shows what each provider proposes and lets you pick the main series", async () => {
    srv.status = status({ resume_step: "coverage", coverage: coverageJob() });
    srv.analysis = coverage([
      provider("metron", "Metron", {
        main_series_id: "1713",
        current_series_id: "1711",
        confidence: "medium",
        confidence_reasons: ["2 candidates list your issues"],
        candidates: [
          {
            provider_series_id: "1711",
            name: "Fantastic Four",
            year: 1961,
            publisher: "Marvel",
            url: "https://metron.cloud/series/1711",
            origin: "search",
            strict: true,
            listed_count: 416,
            local_matches: 160,
            assigned: 160,
            partial: false,
          },
          {
            provider_series_id: "1713",
            name: "Fantastic Four",
            year: 2012,
            publisher: "Marvel",
            url: "https://metron.cloud/series/1713",
            origin: "search",
            strict: true,
            listed_count: 13,
            local_matches: 13,
            assigned: 13,
            partial: false,
          },
        ],
        proposed_ranges: [],
        uncovered: ["700"],
      }),
    ]);
    renderFlow();
    await heading(/Step 1 of 4: Coverage/);
    const row = await waitFor(() =>
      screen.getByTestId("refresh-coverage-metron"),
    );
    // The headline, then the facts the Details tab shows.
    expect(row.textContent).toContain(
      "Metron proposes a different main series",
    );
    expect(row.textContent).toContain("Proposed main");
    expect(row.textContent).toContain("Fantastic Four (2012)");
    expect(row.textContent).toContain("lists 13 issues, 13 of yours");
    expect(row.textContent).toContain("Linked now");
    expect(row.textContent).toContain("Fantastic Four (1961)");
    expect(row.textContent).toContain("2 candidates list your issues");
    expect(screen.getByTestId("coverage-list-metron")).toBeTruthy();
    expect(row.textContent).toContain("No Metron series has #700");
    // More than one candidate → the main-series picker is offered.
    expect(
      screen.getByRole("combobox", { name: "Metron main series" }),
    ).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Accept Metron" }));
    await waitFor(() =>
      expect(sent("POST", "/provider-coverage/accept")).toHaveLength(1),
    );
    // Following the proposal sends null (the server uses its main).
    expect(sent("POST", "/provider-coverage/accept")[0]!.body).toEqual({
      source: "metron",
      main_series_id: null,
    });
  });

  it("accepts a provider whose proposal conflicts with a user range", async () => {
    // The Details tab lets you accept despite a conflict (the server
    // refuses only a differing user-set link); the wizard now matches.
    srv.status = status({ resume_step: "coverage", coverage: coverageJob() });
    srv.analysis = coverage([
      provider("metron", "Metron", {
        conflicts: ["your range #600–611 → #1799 disagrees"],
      }),
    ]);
    renderFlow();
    await heading(/Step 1 of 4: Coverage/);
    const row = await waitFor(() =>
      screen.getByTestId("refresh-coverage-metron"),
    );
    expect(row.textContent).toContain("your range #600–611 → #1799 disagrees");
    expect(screen.getByRole("button", { name: "Accept Metron" })).toBeTruthy();
  });

  it("runs the analysis when no job exists", async () => {
    srv.status = status({ resume_step: "coverage" });
    renderFlow();
    await heading(/Step 1 of 4: Coverage/);
    const run = await waitFor(() =>
      screen.getByRole("button", { name: "Analyze coverage" }),
    );
    srv.analysis = coverage([provider("metron", "Metron")], {
      state: "running",
      trigger: "analyze",
      finished_at: null,
    });
    fireEvent.click(run);
    await waitFor(() =>
      expect(sent("POST", "/provider-coverage/analyze")).toHaveLength(1),
    );
    expect(sent("POST", "/provider-coverage/analyze")[0]!.body).toEqual({
      auto_accept: false,
    });
    await waitFor(() =>
      expect(screen.getByText(/Listing candidate series/)).toBeTruthy(),
    );
  });

  it("says what a capped run covers and offers the remainder from Review", async () => {
    // 247 issues, 200 per run: the fetch step says so, and once the batch
    // finishes the (refetched) estimate says 47 are left → one click more.
    const est = (issues: number, eligible: number, recent: number) => ({
      scope: "all" as const,
      issues,
      eligible,
      recently_fetched: recent,
      remainder: eligible - recent - issues,
      providers: [{ source: "comicvine", direct: issues, search: 0 }],
    });
    srv.status = status({
      resume_step: "fetch",
      fetch_estimate: [est(200, 247, 0), status().fetch_estimate[1]!],
    });
    srv.batchItems = 200;
    srv.batchEligible = 247;
    renderFlow();
    await heading(/Step 3 of 4: Per-issue fetch/);
    fireEvent.click(screen.getByRole("radio", { name: /All issues/ }));
    expect(screen.getByTestId("fetch-cap-note").textContent).toContain(
      "this one covers 200 of 247",
    );
    // The batch finishes; the status now reports the 47 unfetched.
    srv.batch = batch("completed", { items_total: 200 });
    srv.status = status({
      resume_step: "review",
      batch: {
        batch_id: "b1",
        created_at: T2,
        items_total: 200,
        unfinished: 0,
      },
      fetch_estimate: [est(47, 247, 200), status().fetch_estimate[1]!],
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Fetch 200 of 247 issues" }),
    );
    await heading(/Step 4 of 4: Review/);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Fetch the remaining 47 issues" }),
      ).toBeTruthy(),
    );
    expect(screen.getByTestId("review-remaining").textContent).toContain(
      "47 more are still unfetched",
    );
    srv.batchItems = 47;
    srv.batchEligible = 247;
    fireEvent.click(
      screen.getByRole("button", { name: "Fetch the remaining 47 issues" }),
    );
    // Two batch starts (the first run, then the remainder); `all` is the
    // endpoint's default scope, so neither carries a query string.
    await waitFor(() =>
      expect(
        sent("POST", "/series/fantastic-four/metadata/batch"),
      ).toHaveLength(2),
    );
  });

  it("starts an 'Only missing or partial' batch by default", async () => {
    srv.status = status({ resume_step: "fetch" });
    renderFlow();
    await heading(/Step 3 of 4: Per-issue fetch/);
    expect(
      (
        screen.getByRole("radio", {
          name: /Only missing or partial/,
        }) as HTMLButtonElement
      ).getAttribute("data-state"),
    ).toBe("checked");
    expect(
      screen.getByText("ComicVine: 12 direct · 0 searched (≈ 12 requests)"),
    ).toBeTruthy();
    srv.batch = batch("running");
    fireEvent.click(screen.getByRole("button", { name: "Fetch 12 issues" }));
    await waitFor(() =>
      expect(screen.getByText("Searching issues…")).toBeTruthy(),
    );
    const posts = sent("POST", "/series/fantastic-four/metadata/batch");
    expect(posts).toHaveLength(1);
    expect(posts[0]!.url).toContain("scope=incomplete");
    expect(screen.getByText("40 / 173 searched")).toBeTruthy();
  });

  it("hands off to Review with Fill missing and a Review-page link", async () => {
    srv.status = status({
      resume_step: "review",
      batch: {
        batch_id: "b1",
        created_at: T2,
        items_total: 173,
        unfinished: 0,
      },
    });
    srv.batch = batch("completed");
    const onClose = vi.fn();
    renderFlow(onClose);
    await heading(/Step 4 of 4: Review/);
    await waitFor(() =>
      expect(
        screen
          .getByRole("link", { name: "Open in Review" })
          .getAttribute("href"),
      ).toBe("/admin/metadata?tab=review&batch=b1"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Fill missing (2)" }));
    await waitFor(() =>
      expect(sent("POST", "/metadata/batch/b1/apply")).toHaveLength(1),
    );
    expect(sent("POST", "/metadata/batch/b1/apply")[0]!.body).toEqual({
      filter: "all_needs_review",
      mode: "fill_missing",
    });
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onClose).toHaveBeenCalled();
  });
});

describe("<SeriesRefreshFlow> review — Replace all", () => {
  it("offers Replace all, confirms, then replaces needs-review and strong", async () => {
    srv.status = status({
      resume_step: "review",
      batch: {
        batch_id: "b1",
        created_at: T2,
        items_total: 173,
        unfinished: 0,
      },
    });
    srv.batch = batch("completed");
    renderFlow();
    await heading(/Step 4 of 4: Review/);
    // 3 strong + 2 needs-review.
    const open = await waitFor(() =>
      screen.getByRole("button", { name: "Replace all (5)" }),
    );
    fireEvent.click(open);
    const confirm = await waitFor(() =>
      screen.getByRole("button", { name: "Replace all" }),
    );
    expect(screen.getByText(/Replace metadata for 5 issues\?/)).toBeTruthy();
    fireEvent.click(confirm);
    await waitFor(() =>
      expect(sent("POST", "/metadata/batch/b1/apply")).toHaveLength(2),
    );
    const bodies = sent("POST", "/metadata/batch/b1/apply").map((c) => c.body);
    expect(bodies).toEqual([
      { filter: "all_needs_review", mode: "replace_all" },
      { filter: "all_strong", mode: "replace_all" },
    ]);
  });
});

describe("<SeriesRefreshDialog> resume", () => {
  it("reopens where the flow got to", async () => {
    srv.status = status({
      resume_step: "fetch",
      series_match: {
        links: [{ source: "metron", external_id: "1711", set_by: "metron" }],
        latest_run: null,
        applied_at: T0,
      },
      coverage: coverageJob(),
      batch: {
        batch_id: "b1",
        created_at: T2,
        items_total: 173,
        unfinished: 133,
      },
    });
    srv.batch = batch("running");
    const qc = client();
    const ui = (open: boolean) => (
      <QueryClientProvider client={qc}>
        <SeriesRefreshDialog
          open={open}
          onOpenChange={() => undefined}
          seriesSlug="fantastic-four"
          seriesName="Fantastic Four"
          libraryId="lib-1"
        />
      </QueryClientProvider>
    );
    const { rerender } = render(ui(true));
    await heading(/Step 3 of 4: Per-issue fetch/);
    await waitFor(() =>
      expect(screen.getByText("Searching issues…")).toBeTruthy(),
    );
    expect(stepButton(/2\. Confirm series/).textContent).toContain("(done)");
    expect(stepButton(/1\. Coverage/).textContent).toContain("(done)");

    // Closed mid-flow; meanwhile the batch finishes.
    rerender(ui(false));
    await waitFor(() =>
      expect(screen.queryByRole("heading", { name: /Step 3/ })).toBeNull(),
    );
    srv.status = status({
      ...(srv.status as SeriesRefreshStatusResp),
      resume_step: "review",
      batch: {
        batch_id: "b1",
        created_at: T2,
        items_total: 173,
        unfinished: 0,
      },
    });
    srv.batch = batch("completed");

    rerender(ui(true));
    await heading(/Step 4 of 4: Review/);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Accept all strong (3)" }),
      ).toBeTruthy(),
    );
    // Resuming never re-runs a step.
    expect(sent("POST", "/metadata/batch")).toHaveLength(0);
    expect(sent("POST", "/metadata/search")).toHaveLength(0);
  });
});
