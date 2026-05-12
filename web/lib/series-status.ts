import type { SeriesView } from "@/lib/api/types";

export type CollectionStatus = "complete" | "incomplete";

/**
 * Derive whether the user's collection covers the publisher-claimed
 * total. Returns `null` when there's no signal (the server has no
 * `total_issues` for this series — most often because nothing in the
 * series has a ComicInfo `<Count>` yet).
 *
 * Comparison uses **`>=`**, never `===`. Real libraries routinely
 * have more files than `Count` claims (Issue #0 / annuals / variants
 * / a duplicate the user hasn't deduped). Over-collection should
 * still report `"complete"` — the publisher's claim has been met.
 */
export function collectionStatus(
  series: Pick<SeriesView, "issue_count" | "total_issues">,
): CollectionStatus | null {
  const total = series.total_issues;
  if (total == null || total <= 0) return null;
  const have = series.issue_count ?? 0;
  return have >= total ? "complete" : "incomplete";
}
