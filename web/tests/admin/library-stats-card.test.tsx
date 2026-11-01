// @vitest-environment jsdom
/**
 * Admin library overview → "Contents and disk usage" card, and the
 * one-line summary the libraries list shows (audit 2026-11 §5 #16).
 * Figures come from `GET /admin/libraries/{slug}/stats`; a library the
 * backup sweep has not counted yet shows "Not counted yet", not 0 B.
 */
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { LibraryStatsView } from "@/lib/api/types";

const state: { data: LibraryStatsView | undefined; isLoading: boolean } = {
  data: undefined,
  isLoading: false,
};
vi.mock("@/lib/api/queries", () => ({
  useLibraryStats: () => ({ data: state.data, isLoading: state.isLoading }),
}));

import {
  LibraryStatsCard,
  backupSummary,
  libraryStatsSummary,
} from "@/components/admin/library/LibraryStatsCard";

const base: LibraryStatsView = {
  library_id: "00000000-0000-0000-0000-000000000001",
  slug: "main",
  series_count: 1204,
  active_issue_count: 18330,
  removed_issue_count: 0,
  archive_bytes: 812_400_000_000,
  backup_bytes: 3_200_000,
  backup_file_count: 4,
  backup_counted_at: "2026-10-31T03:00:00Z",
  backup_stale: false,
  last_scan_at: "2026-10-31T02:00:00Z",
};

afterEach(() => {
  cleanup();
  state.data = undefined;
  state.isLoading = false;
});

describe("libraryStatsSummary", () => {
  it("joins series, issues and archive size; removed only when present", () => {
    expect(libraryStatsSummary(base)).toBe(
      "1,204 series · 18,330 issues · 812 GB",
    );
    expect(libraryStatsSummary({ ...base, removed_issue_count: 7 })).toBe(
      "1,204 series · 18,330 issues · 812 GB · 7 removed",
    );
  });
});

describe("backupSummary", () => {
  it("shows size and file count, or that no tally exists yet", () => {
    expect(backupSummary(base)).toBe("3.2 MB · 4 files");
    expect(backupSummary({ ...base, backup_file_count: 1 })).toBe(
      "3.2 MB · 1 file",
    );
    expect(
      backupSummary({
        ...base,
        backup_bytes: null,
        backup_file_count: null,
        backup_counted_at: null,
      }),
    ).toBe("Not counted yet");
  });
});

describe("<LibraryStatsCard>", () => {
  it("renders every figure with deep links to the removed and health tabs", () => {
    state.data = { ...base, removed_issue_count: 12 };
    render(<LibraryStatsCard librarySlug="main" />);

    expect(screen.getByText("Contents and disk usage")).toBeTruthy();
    expect(screen.getByText("1,204")).toBeTruthy();
    expect(screen.getByText("18,330")).toBeTruthy();
    expect(screen.getByText("812 GB")).toBeTruthy();

    const removed = screen.getByRole("link", { name: "12" });
    expect(removed.getAttribute("href")).toBe("/admin/libraries/main/removed");
    const backups = screen.getByRole("link", { name: "3.2 MB · 4 files" });
    expect(backups.getAttribute("href")).toBe("/admin/libraries/main/health");
    expect(screen.getByText(/Backups counted/)).toBeTruthy();
    expect(screen.queryByText(/may be behind/)).toBeNull();
  });

  it("flags a tally a scan has made stale", () => {
    state.data = { ...base, backup_stale: true };
    render(<LibraryStatsCard librarySlug="main" />);
    expect(screen.getByText(/may be behind until the next count/)).toBeTruthy();
  });

  it("says backups are not counted yet instead of showing zero", () => {
    state.data = {
      ...base,
      backup_bytes: null,
      backup_file_count: null,
      backup_counted_at: null,
      last_scan_at: null,
    };
    render(<LibraryStatsCard librarySlug="main" />);
    expect(screen.getByRole("link", { name: "Not counted yet" })).toBeTruthy();
    expect(screen.getByText("Never")).toBeTruthy();
    expect(screen.queryByText(/Backups counted/)).toBeNull();
  });

  it("shows a failure line when the figures did not load", () => {
    render(<LibraryStatsCard librarySlug="main" />);
    expect(screen.getByText("Failed to load library figures.")).toBeTruthy();
  });
});
