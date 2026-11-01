import type { SeriesView } from "@/lib/api/types";

export type CollectionStatus = "complete" | "incomplete";

type CountFields = Pick<
  SeriesView,
  "issue_count" | "main_issue_count" | "special_issue_count" | "total_issues"
> &
  Partial<Pick<SeriesView, "specials_count_toward_completion">>;

/**
 * The main run (`main_issue_count`) on the shelf. Falls back to
 * `issue_count` for a payload that predates the split.
 */
export function ownedMainCount(series: CountFields): number {
  return series.main_issue_count ?? series.issue_count ?? 0;
}

/** Annuals / specials on the shelf, or 0 when the payload has no split. */
export function ownedSpecialCount(series: CountFields): number {
  return series.special_issue_count ?? 0;
}

/** Whether this viewer counts the series' annuals / specials toward its
 *  completion (the account setting or the per-series override — the
 *  server resolves it into `specials_count_toward_completion`, B9). */
export function specialsCounted(series: CountFields): boolean {
  return series.specials_count_toward_completion === true;
}

/**
 * Issues that count toward the publisher's `total_issues`: the main run,
 * plus the annuals / one-shots / specials / TPBs when the viewer counts
 * them ({@link specialsCounted}). By default the extras sit on top of the
 * run and never stand in for a missing numbered issue.
 */
export function ownedCompletionCount(series: CountFields): number {
  return (
    ownedMainCount(series) +
    (specialsCounted(series) ? ownedSpecialCount(series) : 0)
  );
}

/** "+1 special" / "+3 specials" — or `null` when there are none. When
 *  the viewer counts specials toward completion they are already in the
 *  owned figure, so the suffix reads "incl. 1 special" instead. */
export function formatSpecialsSuffix(series: CountFields): string | null {
  const n = ownedSpecialCount(series);
  if (n <= 0) return null;
  const noun = n === 1 ? "special" : "specials";
  return specialsCounted(series) ? `incl. ${n} ${noun}` : `+${n} ${noun}`;
}

/**
 * Derive whether the user's collection covers the publisher-claimed
 * total. Returns `null` when there's no signal (the server has no
 * `total_issues` for this series — most often because nothing in the
 * series has a ComicInfo `<Count>` yet).
 *
 * Compares {@link ownedCompletionCount}: the main run, plus the specials
 * when the viewer counts them. By default a 4-of-5 run with an annual on
 * the shelf is still incomplete.
 *
 * Comparison uses **`>=`**, never `===`. Real libraries routinely
 * have more numbered files than `Count` claims (Issue #0 / variants /
 * a duplicate the user hasn't deduped). Over-collection should
 * still report `"complete"` — the publisher's claim has been met.
 */
export function collectionStatus(series: CountFields): CollectionStatus | null {
  const total = series.total_issues;
  if (total == null || total <= 0) return null;
  return ownedCompletionCount(series) >= total ? "complete" : "incomplete";
}

/** Tooltip text shared by the detail badge and the card dot. */
export function collectionTooltip(series: CountFields): string {
  const have = ownedCompletionCount(series);
  const total = series.total_issues ?? 0;
  const base =
    collectionStatus(series) === "complete"
      ? `Complete: ${have} of ${total} issues`
      : `${have} of ${total} issues`;
  const extras = formatSpecialsSuffix(series);
  return extras ? `${base} (${extras})` : base;
}
