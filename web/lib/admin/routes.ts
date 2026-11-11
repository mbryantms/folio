/**
 * Canonical admin URLs for the scan / activity surfaces (scan-activity
 * consolidation, 2026-11). Link here rather than hand-building paths.
 */

/** Canonical "watch a scan" pages. */
export const SCANS_HREF = "/admin/scans";

/** The Scans page scoped to one "Scan all" batch. */
export function scanBatchHref(batchId: string): string {
  return `${SCANS_HREF}?batch=${encodeURIComponent(batchId)}`;
}

/** One library's Scan tab (live progress + run history). */
export function libraryScanHref(slug: string): string {
  return `/admin/libraries/${encodeURIComponent(slug)}/scan`;
}

/** One library's Removed tab (soft-removed series and issues). */
export function libraryRemovedHref(slug: string): string {
  return `/admin/libraries/${encodeURIComponent(slug)}/removed`;
}

/** Server activity → Log view. */
export const SERVER_LOG_HREF = "/admin/activity?view=log";

/** Background work → Jobs view. */
export const JOBS_HREF = "/admin/background-work?view=jobs";
