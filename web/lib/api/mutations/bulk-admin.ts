/**
 * Admin bulk actions for any multi-select surface (bulk-fetch-everywhere):
 * "Fetch metadata" fans a provider search over the selection under one
 * metadata batch (`POST /metadata/batch/selection`), and "Rescan" queues a
 * scoped scan per selected series / issue (`POST /scans/selection`).
 *
 * Both accept issues and series together, from any number of series and
 * libraries, so a mixed collection or a search result is one request.
 *
 * Series-only (phase 2): "Analyze coverage" queues a provider-coverage
 * analysis per series (`POST /metadata/coverage/selection`, proposals
 * only), and "Pause / Resume metadata sync" flips the series' sync flag
 * (`POST /metadata/sync/selection`).
 */
import { useQueryClient } from "@tanstack/react-query";

import { JOBS_HREF, SCANS_HREF } from "@/lib/admin/routes";

import type {
  AnalyzeCoverageSelectionReq,
  AnalyzeCoverageSelectionResp,
  BatchCreatedResp,
  ScanSelectionReq,
  ScanSelectionResp,
  SelectionBatchReq,
  SyncSelectionReq,
  SyncSelectionResp,
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
/** `COVERAGE_SELECTION_MAX_SERIES` (provider_coverage.rs). */
export const COVERAGE_SELECTION_MAX_SERIES = 50;
/** `SYNC_SELECTION_MAX_SERIES` (metadata_search.rs). */
export const SYNC_SELECTION_MAX_SERIES = 100;

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

/** Toast copy for a bulk coverage analysis. */
export function summarizeCoverageSelection(
  resp: AnalyzeCoverageSelectionResp | null,
): string {
  if (!resp) return "Coverage analysis queued";
  const parts: string[] = [];
  if (resp.queued > 0)
    parts.push(
      `Analyzing coverage for ${plural(resp.queued, "series", "series")}`,
    );
  if (resp.already_running > 0)
    parts.push(`${resp.already_running} already running`);
  if (resp.not_found > 0) parts.push(`${resp.not_found} not found`);
  if (resp.failed > 0) parts.push(`${resp.failed} failed`);
  return parts.length > 0 ? parts.join("; ") : "Nothing to analyze.";
}

/** Toast copy for a bulk pause / resume. */
export function summarizeSyncSelection(
  resp: SyncSelectionResp | null,
  paused: boolean,
): string {
  const verb = paused ? "Paused" : "Resumed";
  if (!resp) return `${verb} metadata sync`;
  const parts: string[] = [];
  if (resp.updated > 0)
    parts.push(
      `${verb} metadata sync for ${plural(resp.updated, "series", "series")}`,
    );
  if (resp.unchanged > 0)
    parts.push(`${resp.unchanged} already ${paused ? "paused" : "syncing"}`);
  if (resp.not_found > 0) parts.push(`${resp.not_found} not found`);
  return parts.length > 0 ? parts.join("; ") : "No changes.";
}

/** Analyze provider coverage for selected series (proposals only). */
export function useBulkAnalyzeCoverage() {
  return useApiMutation<
    AnalyzeCoverageSelectionResp,
    AnalyzeCoverageSelectionReq
  >(
    (body) => ({
      path: "/metadata/coverage/selection",
      method: "POST",
      body,
    }),
    {
      successMessage: (resp) => summarizeCoverageSelection(resp),
      successVariant: (resp) => (resp && resp.queued > 0 ? "success" : "info"),
      successAction: (resp) =>
        resp && resp.queued > 0
          ? { label: "View progress", href: JOBS_HREF }
          : undefined,
    },
  );
}

/** Pause or resume metadata sync for selected series. */
export function useBulkSetMetadataSync() {
  const qc = useQueryClient();
  return useApiMutation<SyncSelectionResp, SyncSelectionReq>(
    (body) => ({ path: "/metadata/sync/selection", method: "POST", body }),
    {
      successMessage: (resp, input) =>
        summarizeSyncSelection(resp, input.paused),
      successVariant: (resp) => (resp && resp.updated > 0 ? "success" : "info"),
      onSuccess: () => {
        // Each series' sync-status card + Metadata tab overview.
        qc.invalidateQueries({
          predicate: (q) =>
            q.queryKey[0] === "series" && q.queryKey[2] === "metadata",
        });
      },
    },
  );
}
