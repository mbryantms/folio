// @vitest-environment jsdom
/**
 * Review tab batch header — per-provider direct-lookup vs search counts.
 * A batch child answered from its series' coverage (the provider series'
 * issue list) skips the provider search; the header shows how many did,
 * per provider, and why the rest searched.
 *
 * Real ReviewTab + real QueryClient; only the network (`apiFetch`) is faked.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { BatchLookupCount, BatchStatusResp } from "@/lib/api/types";

const net = vi.hoisted(() => ({ body: null as unknown }));

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(async () => {
    return new Response(JSON.stringify(net.body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }),
  getCsrfToken: () => "csrf",
}));

import { lookupLine } from "@/components/admin/metadata/BatchLookupSummary";
import { ReviewTab } from "@/components/admin/metadata/ReviewTab";

function status(lookups: BatchLookupCount[]): BatchStatusResp {
  return {
    batch_id: "b1",
    scope: "series_issues",
    status: "completed",
    items_total: 12,
    created_at: "2026-10-03T00:00:00Z",
    aggregate: {
      searched: 12,
      strong: 0,
      needs_review: 12,
      no_match: 0,
      applied: 0,
      awaiting_quota: 0,
      failed: 0,
      in_flight: 0,
      partial: 0,
      lookups,
    },
    children: [],
    budget: [],
    exceeds_budget: false,
    resume_eta: null,
  };
}

function renderTab() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ReviewTab initialBatchId="b1" />
    </QueryClientProvider>,
  );
}

describe("Review tab batch lookups", () => {
  it("shows direct vs searched per provider with fallback reasons", async () => {
    net.body = status([
      { source: "comicvine", direct: 10, search: 0, fallbacks: [] },
      {
        source: "metron",
        direct: 9,
        search: 3,
        fallbacks: [
          { reason: "not_listed", count: 2 },
          { reason: "date_conflict", count: 1 },
        ],
      },
      {
        source: "gcd",
        direct: 0,
        search: 12,
        fallbacks: [{ reason: "no_target", count: 12 }],
      },
    ]);
    renderTab();
    const list = await screen.findByRole("list", { name: "Provider lookups" });
    await waitFor(() =>
      expect(list.textContent).toContain("ComicVine: 10 direct · 0 searched"),
    );
    const items = Array.from(list.querySelectorAll("li")).map(
      (li) => li.textContent,
    );
    expect(items).toEqual([
      "ComicVine: 10 direct · 0 searched",
      "Metron: 9 direct · 3 searched (2 number not listed, 1 cover date conflict)",
      "GCD: 0 direct · 12 searched (12 no provider series)",
    ]);
  });

  it("renders nothing before any child has searched", async () => {
    net.body = status([]);
    renderTab();
    await screen.findByText("12 / 12 searched");
    expect(screen.queryByRole("list", { name: "Provider lookups" })).toBeNull();
  });

  it("formats a line without fallbacks", () => {
    expect(
      lookupLine({ source: "metron", direct: 3, search: 0, fallbacks: [] }),
    ).toBe("Metron: 3 direct · 0 searched");
  });
});

describe("candidate coverage reason", () => {
  it("surfaces the coverage note from score_breakdown", async () => {
    const { coverageReason } =
      await import("@/components/library/MetadataMatchCandidates");
    const base = {
      bucket: "medium",
      candidate: {},
      external_id: "44623",
      score: 67.5,
      source: "comicvine",
    };
    expect(
      coverageReason({
        ...base,
        score_breakdown: {
          coverage: {
            reason: "matched by series coverage (number + cover date)",
          },
        },
      }),
    ).toBe("Matched by series coverage (number + cover date)");
    expect(coverageReason({ ...base, score_breakdown: { name: 90 } })).toBe(
      null,
    );
  });
});
