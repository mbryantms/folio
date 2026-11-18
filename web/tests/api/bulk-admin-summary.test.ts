/**
 * Toast copy for the admin bulk actions every multi-select surface shares
 * (Fetch metadata → `POST /metadata/batch/selection`, Rescan →
 * `POST /scans/selection`).
 */
import { describe, expect, it } from "vitest";

import {
  reviewBatchHref,
  summarizeScanSelection,
  summarizeSelectionBatch,
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
