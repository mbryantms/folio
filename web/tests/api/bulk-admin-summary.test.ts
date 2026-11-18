/**
 * Toast copy for the admin bulk actions every multi-select surface shares
 * (Fetch metadata → `POST /metadata/batch/selection`, Rescan →
 * `POST /scans/selection`).
 */
import { describe, expect, it } from "vitest";

import {
  reviewBatchHref,
  summarizeCoverageSelection,
  summarizeScanSelection,
  summarizeSelectionBatch,
  summarizeSyncSelection,
  summarizeWantToReadAdd,
} from "@/lib/api/mutations";
import type { BatchCreatedResp } from "@/lib/api/types";

function batch(over: Partial<BatchCreatedResp>): BatchCreatedResp {
  return {
    batch_id: "b-1",
    items_total: 0,
    jobs_enqueued: 0,
    jobs_coalesced: 0,
    jobs_failed: 0,
    eligible: 0,
    recently_fetched: 0,
    remainder: 0,
    ...over,
  };
}

describe("summarizeSelectionBatch", () => {
  it("counts the searches and the extras", () => {
    expect(
      summarizeSelectionBatch(batch({ items_total: 1, jobs_enqueued: 1 })),
    ).toBe("Searching 1 item for metadata");
    expect(
      summarizeSelectionBatch(
        batch({ items_total: 5, jobs_coalesced: 2, jobs_failed: 1 }),
      ),
    ).toBe(
      "Searching 5 items for metadata; 2 already in progress; 1 failed to queue",
    );
  });

  it("explains an empty batch", () => {
    expect(summarizeSelectionBatch(batch({ jobs_coalesced: 3 }))).toMatch(
      /already being searched/,
    );
    expect(summarizeSelectionBatch(batch({ jobs_failed: 2 }))).toMatch(
      /Couldn't queue/,
    );
    expect(summarizeSelectionBatch(batch({}))).toBe(
      "Nothing to search in that selection.",
    );
    expect(summarizeSelectionBatch(null)).toBe("Queued metadata search");
  });

  it("links the Review tab to the batch", () => {
    expect(reviewBatchHref("abc")).toBe("/admin/metadata?tab=review&batch=abc");
  });
});

describe("summarizeScanSelection", () => {
  it("lists every non-zero bucket", () => {
    expect(
      summarizeScanSelection({
        queued: 3,
        coalesced: 1,
        not_found: 2,
        failed: 0,
      }),
    ).toBe("Queued 3 scans; 1 already queued; 2 not found");
    expect(
      summarizeScanSelection({
        queued: 1,
        coalesced: 0,
        not_found: 0,
        failed: 1,
      }),
    ).toBe("Queued 1 scan; 1 failed");
  });

  it("says so when nothing was queued", () => {
    expect(
      summarizeScanSelection({
        queued: 0,
        coalesced: 0,
        not_found: 0,
        failed: 0,
      }),
    ).toBe("Nothing to rescan.");
    expect(summarizeScanSelection(null)).toBe("Rescan queued");
  });
});

describe("summarizeCoverageSelection", () => {
  it("counts queued and already-running analyses", () => {
    expect(
      summarizeCoverageSelection({
        queued: 2,
        already_running: 1,
        not_found: 0,
        failed: 0,
      }),
    ).toBe("Analyzing coverage for 2 series; 1 already running");
    expect(
      summarizeCoverageSelection({
        queued: 0,
        already_running: 0,
        not_found: 0,
        failed: 0,
      }),
    ).toBe("Nothing to analyze.");
  });
});

describe("summarizeSyncSelection", () => {
  it("names the direction and the no-ops", () => {
    expect(
      summarizeSyncSelection({ updated: 3, unchanged: 1, not_found: 0 }, true),
    ).toBe("Paused metadata sync for 3 series; 1 already paused");
    expect(
      summarizeSyncSelection({ updated: 1, unchanged: 2, not_found: 1 }, false),
    ).toBe(
      "Resumed metadata sync for 1 series; 2 already syncing; 1 not found",
    );
    expect(
      summarizeSyncSelection({ updated: 0, unchanged: 0, not_found: 0 }, true),
    ).toBe("No changes.");
  });
});

describe("summarizeWantToReadAdd", () => {
  it("reports what landed", () => {
    expect(
      summarizeWantToReadAdd({
        added: 4,
        already_present: 1,
        not_found: 1,
        invalid: 0,
      }),
    ).toBe("Added 4 to Want to Read; 1 already there; 1 skipped");
    expect(summarizeWantToReadAdd(null)).toBe("Added to Want to Read");
  });
});
