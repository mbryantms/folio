/**
 * Multi-select M7: toast-summary helpers for the bulk-progress
 * mutations. Pure functions extracted from `useBulkMarkProgress` /
 * `useBulkMarkSeriesProgress` so the messaging is testable without
 * spinning up TanStack Query or `useApiMutation`.
 *
 * Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M7).
 */
import { describe, expect, it } from "vitest";

import {
  summarizeBulkProgress,
  summarizeBulkSeriesProgress,
} from "@/lib/api/mutations";

describe("summarizeBulkProgress", () => {
  it("falls back to a verb-only message when data is missing", () => {
    expect(summarizeBulkProgress(undefined, true)).toBe("Marked read");
    expect(summarizeBulkProgress(undefined, false)).toBe("Marked unread");
  });

  it("reports `N marked read` for a clean success", () => {
    expect(
      summarizeBulkProgress(
        { updated: 3, skipped: 0, forbidden: 0, not_found: 0 },
        true,
      ),
    ).toBe("3 marked read");
  });

  it("reports `N marked unread` for the unread variant", () => {
    expect(
      summarizeBulkProgress(
        { updated: 2, skipped: 0, forbidden: 0, not_found: 0 },
        false,
      ),
    ).toBe("2 marked unread");
  });

  it("appends `M already read` when some were skipped", () => {
    expect(
      summarizeBulkProgress(
        { updated: 1, skipped: 2, forbidden: 0, not_found: 0 },
        true,
      ),
    ).toBe("1 marked read; 2 already read");
  });

  it("appends `M already unread` for the unread variant", () => {
    expect(
      summarizeBulkProgress(
        { updated: 0, skipped: 3, forbidden: 0, not_found: 0 },
        false,
      ),
    ).toBe("3 already unread");
  });

  it("combines forbidden + not_found into a single `skipped` trailer", () => {
    expect(
      summarizeBulkProgress(
        { updated: 1, skipped: 0, forbidden: 1, not_found: 2 },
        true,
      ),
    ).toBe("1 marked read; 3 skipped");
  });

  it("returns `No changes` when every bucket is zero", () => {
    expect(
      summarizeBulkProgress(
        { updated: 0, skipped: 0, forbidden: 0, not_found: 0 },
        true,
      ),
    ).toBe("No changes");
  });
});

describe("summarizeBulkSeriesProgress", () => {
  it("reports issue-level updated + series-level skipped", () => {
    // 2 series mark-read, one expanded to 18 issues, one was missing.
    expect(
      summarizeBulkSeriesProgress(
        {
          updated: 18,
          skipped: 0,
          forbidden_series: 0,
          not_found_series: 1,
        },
        true,
      ),
    ).toBe("18 marked read; 1 series skipped");
  });

  it("trailer reads `series skipped` not just `skipped`", () => {
    // Distinguishes a series-level drop from the issue-level skip
    // bucket — important because the user picked series, not issues.
    expect(
      summarizeBulkSeriesProgress(
        {
          updated: 0,
          skipped: 0,
          forbidden_series: 2,
          not_found_series: 0,
        },
        false,
      ),
    ).toBe("2 series skipped");
  });

  it("returns `No changes` when nothing happened on a series-bulk call", () => {
    expect(
      summarizeBulkSeriesProgress(
        {
          updated: 0,
          skipped: 0,
          forbidden_series: 0,
          not_found_series: 0,
        },
        true,
      ),
    ).toBe("No changes");
  });

  it("falls back to a verb-only message when data is missing", () => {
    expect(summarizeBulkSeriesProgress(undefined, true)).toBe("Marked read");
  });
});
