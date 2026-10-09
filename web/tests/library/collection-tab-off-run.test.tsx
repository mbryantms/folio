// @vitest-environment jsdom
/**
 * Series Collection tab with an off-run stunt number: The Flash (1987)
 * owns #0–#247 and DC One Million's #1,000,000. The server reports the
 * stunt number in `main_run.off_run` and stops the run at #247, so the
 * grid renders chips only up to #247 and the stunt number appears under
 * "Specials & extras" instead of inflating the grid to a million cells.
 */
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { CollectionReportView } from "@/lib/api/types";

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
    slug: `flash-${n}`,
    number_raw: String(n),
    title: null,
    sort_number: n,
    special_type: null,
    metadata_tier: "complete",
    missing_core: [],
  };
}

describe("CollectionTab with an off-run number", () => {
  it("keeps the grid at the run's max and lists the stunt number as an extra", () => {
    const run = [0, 1, 2, 3, 4];
    const owned = [...run, 1_000_000];
    report = {
      total_owned: owned.length,
      total_expected: 250,
      completeness_pct: 2.4,
      completeness_state: "incomplete",
      main_run: {
        present: run,
        present_labels: run.map(String),
        missing: [],
        possibly_missing: [],
        off_run: [1_000_000],
        min: 0,
        max: 4,
        trailing_missing: 244,
      },
      specials: [
        { number_raw: "1000000", sort_number: 1_000_000, special_type: null },
      ],
      expected_source: "series_total",
      manifest: null,
      issues: owned.map(issue),
    } as CollectionReportView;

    render(<CollectionTab seriesSlug="the-flash-1987" />);

    // Grid: exactly #0–#4, no interpolation towards #1,000,000.
    for (const n of run) {
      expect(screen.getByTitle(`Issue #${n} — Complete`)).toBeTruthy();
    }
    expect(screen.queryByTitle(/Issue #1000000/)).toBeNull();
    expect(screen.queryByTitle(/Issue #5 /)).toBeNull();
    // Trailing count comes from the server, unchanged by the stunt number.
    expect(screen.getByText("+244")).toBeTruthy();

    // The stunt number lives under Specials & extras.
    expect(screen.getByText("Specials & extras")).toBeTruthy();
    expect(screen.getByTitle("Special — Complete").textContent).toContain(
      "1000000",
    );
  });
});
