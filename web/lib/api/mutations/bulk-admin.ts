/**
 * Admin bulk actions for any multi-select surface (bulk-fetch-everywhere):
 * "Fetch metadata" fans a provider search over the selection under one
 * metadata batch (`POST /metadata/batch/selection`), and "Rescan" queues a
 * scoped scan per selected series / issue (`POST /scans/selection`).
 *
 * Both accept issues and series together, from any number of series and
 * libraries, so a mixed collection or a search result is one request.
 */
import { SCANS_HREF } from "@/lib/admin/routes";

import type {
  BatchCreatedResp,
  ScanSelectionReq,
  ScanSelectionResp,
  SelectionBatchReq,
} from "../types";
import { useApiMutation } from "./_core";

/** Server caps, mirrored so the toolbar can disable an over-cap action
 *  instead of letting the request 422. Keep in step with
 *  `SELECTION_BATCH_MAX_*` (metadata_search.rs) and `SCAN_SELECTION_MAX_*`
 *  (scan_selection.rs). */
export const FETCH_SELECTION_MAX_ISSUES = 200;
export const FETCH_SELECTION_MAX_SERIES = 100;
export const RESCAN_SELECTION_MAX_ISSUES = 500;
export const RESCAN_SELECTION_MAX_SERIES = 100;

export function reviewBatchHref(batchId: string): string {
  return `/admin/metadata?tab=review&batch=${encodeURIComponent(batchId)}`;
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Toast copy for a selection batch. */
export function summarizeSelectionBatch(resp: BatchCreatedResp | null): string {
  if (!resp) return "Queued metadata search";
  if (resp.items_total === 0) {
    if (resp.jobs_coalesced > 0)
      return "Nothing new to search — those are already being searched.";
    if (resp.jobs_failed > 0) return "Couldn't queue any metadata searches.";
    return "Nothing to search in that selection.";
  }
  const parts = [`Searching ${plural(resp.items_total, "item")} for metadata`];
  if (resp.jobs_coalesced > 0)
    parts.push(`${resp.jobs_coalesced} already in progress`);
  if (resp.jobs_failed > 0) parts.push(`${resp.jobs_failed} failed to queue`);
  return parts.join("; ");
}

/** Toast copy for a bulk rescan. */
export function summarizeScanSelection(resp: ScanSelectionResp | null): string {
  if (!resp) return "Rescan queued";
  const parts: string[] = [];
  if (resp.queued > 0) parts.push(`Queued ${plural(resp.queued, "scan")}`);
  if (resp.coalesced > 0) parts.push(`${resp.coalesced} already queued`);
  if (resp.not_found > 0) parts.push(`${resp.not_found} not found`);
  if (resp.failed > 0) parts.push(`${resp.failed} failed`);
  return parts.length > 0 ? parts.join("; ") : "Nothing to rescan.";
}

/** Fetch metadata for a selection → one batch, toast links to Review. */
export function useBulkMetadataFetch() {
  return useApiMutation<BatchCreatedResp, SelectionBatchReq>(
    (body) => ({ path: "/metadata/batch/selection", method: "POST", body }),
    {
      successMessage: (resp) => summarizeSelectionBatch(resp),
      successVariant: (resp) =>
        resp && resp.items_total > 0 ? "success" : "info",
      successAction: (resp) =>
        resp && resp.items_total > 0
          ? { label: "Review results", href: reviewBatchHref(resp.batch_id) }
          : undefined,
    },
  );
}

/** Rescan a selection (fast path unless `force`). */
export function useBulkRescan() {
  return useApiMutation<ScanSelectionResp, ScanSelectionReq>(
    (body) => ({ path: "/scans/selection", method: "POST", body }),
    {
      successMessage: (resp) => summarizeScanSelection(resp),
      successVariant: (resp) => (resp && resp.queued > 0 ? "success" : "info"),
      successAction: (resp) =>
        resp && resp.queued > 0
          ? { label: "View scans", href: SCANS_HREF }
          : undefined,
    },
  );
}
