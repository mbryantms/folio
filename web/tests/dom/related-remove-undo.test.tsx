// @vitest-environment jsdom
/**
 * "Undo instead of confirm" on the Related tab: removing a relationship
 * or an admin-added external link happens at once (no AlertDialog) and
 * the Undo toast re-creates it from the row's own data. A provider link
 * keeps its confirm (covered in rail-ring-room.test.tsx).
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SeriesRelationshipsResp, SeriesView } from "@/lib/api/types";

const m = vi.hoisted(() => ({
  del: vi.fn(),
  create: vi.fn(async () => ({})),
  delExt: vi.fn(),
  createExt: vi.fn(async () => ({})),
  toastSuccess: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: m.toastSuccess, error: vi.fn() }),
}));

let data: SeriesRelationshipsResp | undefined;

vi.mock("@/lib/api/queries", () => ({
  useRelationshipKinds: () => ({ data: undefined }),
  useMe: () => ({ data: { role: "admin" } }),
  useSeriesRelationships: () => ({ data, isLoading: false }),
  useSeriesListInfinite: () => ({ data: undefined, isLoading: false }),
  useEntityListInfinite: () => ({ data: undefined, isLoading: false }),
  useSeriesRelationshipSuggestions: () => ({
    data: undefined,
    isLoading: false,
  }),
}));
vi.mock("@/lib/api/queries-me", () => ({
  useMe: () => ({ data: { role: "admin" } }),
}));
vi.mock("@/lib/api/mutations", () => ({
  useCreateSeriesRelationship: () => ({
    mutate: vi.fn(),
    mutateAsync: m.create,
    isPending: false,
  }),
  useDeleteSeriesRelationship: () => ({ mutate: m.del, isPending: false }),
  useUpdateSeriesRelationship: () => ({ mutate: vi.fn(), isPending: false }),
  useCreateExternalRelationship: () => ({
    mutate: vi.fn(),
    mutateAsync: m.createExt,
    isPending: false,
  }),
  useDeleteExternalRelationship: () => ({ mutate: m.delExt, isPending: false }),
  useAcceptRelationshipSuggestion: () => ({
    mutate: vi.fn(),
    isPending: false,
  }),
  useRejectRelationshipSuggestion: () => ({
    mutate: vi.fn(),
    isPending: false,
  }),
}));

import { SeriesRelatedSection } from "@/components/library/SeriesRelatedSection";

const vol3 = {
  id: "s3",
  slug: "saga-vol-3",
  name: "Saga Vol 3",
  year: 2016,
  library_id: "lib",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  cover_url: null,
} as unknown as SeriesView;

beforeEach(() => {
  for (const f of Object.values(m)) f.mockClear();
  data = {
    series_id: "s2",
    relationships: [
      {
        id: "r3",
        kind: "continued_by",
        kind_label: "Continued by",
        group: "publication",
        source: "manual",
        confidence: null,
        qualifier: "direct",
        from_range: "1-6",
        to_range: null,
        note: "after the time skip",
        created_at: "2026-01-01T00:00:00Z",
        series: vol3,
      },
    ],
    arcs: [],
    external: [
      {
        id: "x1",
        kind: "sequel_of",
        kind_label: "Sequel of",
        group: "story",
        name: "Paper Girls",
        year: 2015,
        provider_series_id: "42",
        source: "metron",
        source_label: "Metron",
        set_by: "user",
        confidence: null,
        created_at: "2026-01-01T00:00:00Z",
      },
    ],
    chain: [],
  } as unknown as SeriesRelationshipsResp;
});

function clickUndo() {
  const opts = m.toastSuccess.mock.calls[0]?.[1] as {
    action: { label: string; onClick: () => void };
  };
  expect(opts.action.label).toBe("Undo");
  opts.action.onClick();
}

describe("Related tab removals", () => {
  it("removes a relationship at once and Undo re-creates it with its scope", async () => {
    render(<SeriesRelatedSection seriesSlug="saga-vol-2" seriesId="s2" />);
    fireEvent.click(
      screen.getByRole("button", { name: "Remove continued by Saga Vol 3" }),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(m.del).toHaveBeenCalledTimes(1);
    expect(m.del.mock.calls[0]?.[0]).toEqual({
      id: "r3",
      otherSlug: "saga-vol-3",
      arcSlug: undefined,
    });
    m.del.mock.calls[0]?.[1]?.onSuccess?.();
    await act(async () => clickUndo());
    expect(m.create).toHaveBeenCalledWith({
      kind: "continued_by",
      target: "s3",
      qualifier: "direct",
      coverage: null,
      from_range: "1-6",
      to_range: null,
      note: "after the time skip",
    });
  });

  it("removes an admin-added external link at once and Undo re-adds it", async () => {
    render(<SeriesRelatedSection seriesSlug="saga-vol-2" seriesId="s2" />);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Remove sequel of Paper Girls (2015)",
      }),
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(m.delExt).toHaveBeenCalledWith({ id: "x1" }, expect.anything());
    m.delExt.mock.calls[0]?.[1]?.onSuccess?.();
    await act(async () => clickUndo());
    expect(m.createExt).toHaveBeenCalledWith({
      kind: "sequel_of",
      name: "Paper Girls",
      provider_series_id: "42",
      qualifier: null,
      source: "metron",
      year: 2015,
    });
  });
});
