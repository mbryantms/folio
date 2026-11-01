// @vitest-environment jsdom
/**
 * External-id add / unlink are admin writes (audit 2026-11 E13: the
 * server's routes take `RequireAdmin`). Readers still see every id, on
 * the issue page's `<ExternalIdsCard>` and on the series Metadata tab's
 * provider-mapping card, but get no "Add ID" or remove controls.
 */
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ExternalIdRow } from "@/lib/api/types";

let role: "admin" | "user" = "admin";
let rows: ExternalIdRow[] = [];
const { idle } = vi.hoisted(() => ({
  idle: () => ({ mutate: () => {}, isPending: false }),
}));

vi.mock("@/lib/api/queries", () => ({
  useMe: () => ({ data: { role } }),
  useExternalIdsSeries: () => ({
    data: { entity_type: "series", entity_id: "s1", rows },
    isLoading: false,
  }),
  useExternalIdsIssue: () => ({
    data: { entity_type: "issue", entity_id: "i1", rows },
    isLoading: false,
  }),
  useProviderCoverageSeries: () => ({
    data: { providers: [] },
    isLoading: false,
  }),
  useProviderCoverageAnalysis: () => ({ data: undefined }),
}));

vi.mock("@/lib/api/mutations", () => ({
  useAddExternalIdSeries: idle,
  useDeleteExternalIdSeries: idle,
  useAddExternalIdIssue: idle,
  useDeleteExternalIdIssue: idle,
  useAcceptProviderCoverage: idle,
  useAddProviderRangeSeries: idle,
  useAnalyzeProviderCoverage: idle,
  useDeleteProviderRangeSeries: idle,
}));

vi.mock("@/components/library/ProviderCoverageAnalysis", () => ({
  ProviderCoverageAnalysis: () => null,
  SERIES_COLORS: ["210 70% 50%"],
}));

vi.mock("@/components/library/CoverageAfterMatchPrompt", () => ({
  CoverageAfterMatchPrompt: () => null,
}));

import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { SeriesProviderRangesCard } from "@/components/library/SeriesProviderRangesCard";

const row = (source: string, label: string, id: string): ExternalIdRow => ({
  source,
  source_label: label,
  external_id: id,
  external_url: null,
  set_by: "comicinfo",
  first_set_at: "2026-10-06T00:00:00Z",
  last_synced_at: "2026-10-06T00:00:00Z",
});

beforeEach(() => {
  role = "admin";
  rows = [
    row("comicvine", "ComicVine", "4050-1"),
    row("marvel", "Marvel", "77"),
  ];
});

describe("<ExternalIdsCard> edit controls", () => {
  for (const chrome of ["bare", "card"] as const) {
    it(`${chrome}: an admin can add and remove ids`, () => {
      render(
        <ExternalIdsCard
          entityType="issue"
          seriesSlug="saga"
          issueSlug="1"
          chrome={chrome}
          canEdit
        />,
      );
      expect(screen.getByText("4050-1")).toBeTruthy();
      expect(
        screen.getByRole("button", { name: /add (id|identifier)/i }),
      ).toBeTruthy();
      expect(
        screen.getByRole("button", { name: "Remove ComicVine link" }),
      ).toBeTruthy();
    });

    it(`${chrome}: a reader sees the ids without add or remove`, () => {
      render(
        <ExternalIdsCard
          entityType="issue"
          seriesSlug="saga"
          issueSlug="1"
          chrome={chrome}
          canEdit={false}
        />,
      );
      expect(screen.getByText("4050-1")).toBeTruthy();
      expect(screen.getByText("77")).toBeTruthy();
      expect(
        screen.queryByRole("button", { name: /add (id|identifier)/i }),
      ).toBeNull();
      expect(
        screen.queryByRole("button", { name: /remove .* link/i }),
      ).toBeNull();
    });
  }
});

describe("<SeriesProviderRangesCard> id controls", () => {
  it("offers an admin Add ID, unlink and remove", () => {
    render(<SeriesProviderRangesCard seriesSlug="saga" />);
    expect(screen.getByRole("button", { name: /add id/i })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Unlink ComicVine" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Remove Marvel link" }),
    ).toBeTruthy();
  });

  it("shows a reader the ids without Add ID, unlink or remove", () => {
    role = "user";
    render(<SeriesProviderRangesCard seriesSlug="saga" />);
    expect(screen.getByText("4050-1")).toBeTruthy();
    expect(screen.getByText("Marvel 77")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /add id/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /^unlink /i })).toBeNull();
    expect(
      screen.queryByRole("button", { name: /remove .* link/i }),
    ).toBeNull();
  });
});
