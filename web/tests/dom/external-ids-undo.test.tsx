// @vitest-environment jsdom
/**
 * Unlinking an external id an admin added by hand happens at once with an
 * Undo that re-adds the same source + id + URL. An id that came from
 * ComicInfo or a provider keeps the confirm: re-adding it would pin it
 * as user-set, which isn't a faithful restore.
 */
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ExternalIdRow } from "@/lib/api/types";

const m = vi.hoisted(() => ({
  del: vi.fn(),
  add: vi.fn(async () => ({})),
  toastSuccess: vi.fn(),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: m.toastSuccess, error: vi.fn() }),
}));

let rows: ExternalIdRow[] = [];
vi.mock("@/lib/api/queries", () => ({
  useExternalIdsSeries: () => ({ data: { rows }, isLoading: false }),
  useExternalIdsIssue: () => ({ data: { rows }, isLoading: false }),
}));
vi.mock("@/lib/api/mutations", () => {
  const hook = () => ({
    mutate: m.del,
    mutateAsync: m.add,
    isPending: false,
  });
  return {
    useAddExternalIdSeries: hook,
    useDeleteExternalIdSeries: hook,
    useAddExternalIdIssue: hook,
    useDeleteExternalIdIssue: hook,
  };
});

import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";

const row = (set_by: string): ExternalIdRow => ({
  source: "comicvine",
  source_label: "ComicVine",
  external_id: "4050-1",
  external_url: "https://comicvine.example/4050-1",
  set_by,
  first_set_at: "2026-10-06T00:00:00Z",
  last_synced_at: "2026-10-06T00:00:00Z",
});

beforeEach(() => {
  for (const f of Object.values(m)) f.mockClear();
});

describe("<ExternalIdsCard> unlink", () => {
  it("unlinks a user-set id at once and Undo re-adds it", async () => {
    rows = [row("user")];
    render(<ExternalIdsCard entityType="series" seriesSlug="saga" canEdit />);
    fireEvent.click(screen.getByRole("button", { name: /comicvine/i }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(m.del).toHaveBeenCalledWith(
      { source: "comicvine" },
      expect.anything(),
    );
    m.del.mock.calls[0]?.[1]?.onSuccess?.();
    const opts = m.toastSuccess.mock.calls[0]?.[1] as {
      action: { onClick: () => void };
    };
    await act(async () => opts.action.onClick());
    expect(m.add).toHaveBeenCalledWith({
      source: "comicvine",
      external_id: "4050-1",
      external_url: "https://comicvine.example/4050-1",
    });
  });

  it("keeps the confirm for an id that came from ComicInfo", () => {
    rows = [row("comicinfo")];
    render(<ExternalIdsCard entityType="series" seriesSlug="saga" canEdit />);
    fireEvent.click(screen.getByRole("button", { name: /comicvine/i }));
    expect(m.del).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeTruthy();
  });
});
