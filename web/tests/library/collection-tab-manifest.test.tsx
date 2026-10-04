// @vitest-environment jsdom
/**
 * Series Collection tab + the provider manifest (coverage tie-ins): with
 * `expected_source = "provider_manifest"` the grid shows owned + provider-
 * listed numbers only (no interpolated #71–499 for a folder holding #1–70
 * and #500+), marks possibly-missing numbers and explains each provider's
 * view; a "not loaded" note is shown as-is; the interpolated fallback is
 * unchanged.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { CollectionReportView } from "@/lib/api/types";
import { collectionRunChips } from "@/lib/collection-manifest";

let report: CollectionReportView;
vi.mock("@/lib/api/queries", () => ({
  useSeriesCollection: () => ({
    data: report,
    isLoading: false,
    isError: false,
  }),
}));
vi.mock("next/link", () => ({
  default: ({
    href,
    children,
    ...rest
  }: {
    href: string;
    children: unknown;
  }) => (
    <a href={href} {...rest}>
      {children as never}
    </a>
  ),
}));

import { CollectionTab } from "@/app/[locale]/(library)/series/[slug]/CollectionTab";

function issue(n: number) {
  return {
    slug: `ff-${n}`,
    number_raw: String(n),
    title: null,
    sort_number: n,
    special_type: null,
    metadata_tier: "complete",
    missing_core: [],
  };
}

function base(nums: number[]): CollectionReportView {
  return {
    total_owned: nums.length,
    total_expected: null,
    completeness_pct: null,
    completeness_state: "unknown",
    main_run: {
      present: nums,
      present_labels: nums.map(String),
      missing: [],
      possibly_missing: [],
      min: Math.min(...nums),
      max: Math.max(...nums),
      trailing_missing: 0,
    },
    specials: [],
    expected_source: "series_total",
    manifest: null,
    issues: nums.map(issue),
  } as CollectionReportView;
}

const OWNED = [1, 2, 3, 500, 501, 600];

describe("collectionRunChips", () => {
  it("interpolates without a manifest and lists only known numbers with one", () => {
    const interp = base([1, 2, 5]);
    interp.main_run.missing = [3, 4];
    expect(collectionRunChips(interp, [1, 2, 5])).toEqual([1, 2, 3, 4, 5]);
    const exact = base(OWNED);
    exact.expected_source = "provider_manifest";
    exact.main_run.missing = [4];
    exact.main_run.possibly_missing = [502];
    expect(collectionRunChips(exact, OWNED)).toEqual([
      1, 2, 3, 4, 500, 501, 502, 600,
    ]);
  });
});

describe("<CollectionTab> provider manifest", () => {
  beforeEach(() => {
    report = base(OWNED);
  });

  it("keeps the interpolated grid without a manifest", () => {
    report.main_run.missing = Array.from({ length: 596 }, (_, i) => i + 4);
    report.main_run.missing = report.main_run.missing.filter(
      (n) => !OWNED.includes(n),
    );
    render(<CollectionTab seriesSlug="ff" />);
    expect(screen.getByText(/\(inferred\)/)).toBeTruthy();
    expect(screen.getByTitle("Issue #71 — Missing")).toBeTruthy();
  });

  it("shows exact gaps, possibly-missing chips and each provider's view", async () => {
    report.expected_source = "provider_manifest";
    report.main_run.missing = [4];
    report.main_run.possibly_missing = [502];
    report.manifest = {
      used: true,
      providers: [],
      missing: ["4", "605.1"],
      possibly_missing: [
        {
          number: "502",
          providers: [
            { source: "comicvine", listing: "listed" },
            { source: "metron", listing: "not_listed" },
            { source: "gcd", listing: "not_loaded" },
          ],
        },
      ],
      note: "GCD provider list not loaded — run Analyze coverage",
    } as CollectionReportView["manifest"];
    render(<CollectionTab seriesSlug="ff" />);
    expect(screen.getByText(/from provider issue lists/)).toBeTruthy();
    expect(screen.getByText(/1 possibly missing/)).toBeTruthy();
    // No interpolated chip for #71.
    expect(screen.queryByTitle("Issue #71 — Missing")).toBeNull();
    expect(screen.getByTitle("Issue #4 — Missing")).toBeTruthy();
    expect(screen.getByTestId("collection-manifest-note").textContent).toBe(
      "GCD provider list not loaded — run Analyze coverage",
    );
    expect(screen.getByText("Also missing: #605.1")).toBeTruthy();

    fireEvent.click(screen.getByTitle("Issue #502 — Possibly missing"));
    await waitFor(() =>
      expect(screen.getByTestId("possibly-missing-providers")).toBeTruthy(),
    );
    const views = screen.getByTestId("possibly-missing-providers").textContent;
    expect(views).toContain("ComicVine: lists it");
    expect(views).toContain("Metron: doesn’t list it");
    expect(views).toContain("GCD: list not loaded");
  });
});
