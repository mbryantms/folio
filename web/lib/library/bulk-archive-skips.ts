import type { BulkEditResponse } from "@/lib/api/types";

/** One skipped issue from a bulk-archive-edit response. */
export type BulkArchiveSkip = BulkEditResponse["skipped"][number];

/**
 * Group bulk-archive-edit skips by their server-supplied reason and render a
 * compact `"2 archive writeback disabled for this library; 1 unsupported
 * archive format (CBZ/CBT/CBR only)"` summary.
 *
 * Audit B17: the dialog used to report only "M skipped" — the operator never
 * learned *why*. We group by the raw reason string (already human-readable,
 * built in `archive_edit::bulk_edit`) rather than mapping to short labels so a
 * server-side wording change can't silently fall through to an empty summary.
 * Highest-count reason first; ties keep first-seen order (stable sort).
 */
export function summarizeBulkArchiveSkips(
  skipped: readonly BulkArchiveSkip[],
): string {
  const counts = new Map<string, number>();
  for (const s of skipped) {
    counts.set(s.reason, (counts.get(s.reason) ?? 0) + 1);
  }
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(([reason, n]) => `${n} ${reason}`)
    .join("; ");
}

/** The issue ids that were skipped — what stays selected for retry. */
export function skippedIssueIds(skipped: readonly BulkArchiveSkip[]): string[] {
  return skipped.map((s) => s.issue_id);
}

/**
 * Map skipped *issue* ids back to the *entry* ids that own them, preserving
 * `entries` order. The CBL + Collection surfaces select by entry id but submit
 * the resolved issue ids, so to keep the skipped items selected we reverse the
 * mapping. Entries without a resolved issue (placeholders / series cards) carry
 * a nullish `issueId` and never match.
 */
export function skippedEntryIds(
  skipped: readonly BulkArchiveSkip[],
  entries: readonly { entryId: string; issueId: string | null | undefined }[],
): string[] {
  const skippedSet = new Set(skipped.map((s) => s.issue_id));
  const out: string[] = [];
  for (const e of entries) {
    if (e.issueId && skippedSet.has(e.issueId)) out.push(e.entryId);
  }
  return out;
}
