import type { IssueSummaryView } from "@/lib/api/types";

/**
 * The user-facing read state of an issue. Drives the dynamic "Read /
 * Continue reading / Read again" button on the issue + series pages, and
 * gates the "Read from beginning" affordance.
 */
export type ReadState = "unread" | "in_progress" | "finished";

/** RFC 3339 string lexicographic compare is monotonic, so it doubles as a
 *  cheap "newer-than" check for sort. */
function isFinished(p: ProgressLike): boolean {
  return p.finished;
}

/** Subset of `ProgressView` used by these helpers so callers can pass the
 *  raw delta records straight in (the existing pages already deserialize
 *  to `{ issue_id, page, finished, updated_at }`). */
export type ProgressLike = {
  issue_id: string;
  page: number;
  finished: boolean;
  updated_at?: string;
};

/** Classify a single issue's read state from its (optional) progress row. */
export function readStateFor(
  issue: { page_count?: number | null },
  progress: ProgressLike | null | undefined,
): ReadState {
  if (!progress) return "unread";
  if (isFinished(progress)) return "finished";
  if (progress.page <= 0) return "unread";
  return "in_progress";
}

/** Button label for the primary read CTA. Matches the spec the user
 *  requested: Read / Continue reading / Read again. */
export function readButtonLabel(state: ReadState): string {
  switch (state) {
    case "unread":
      return "Read";
    case "in_progress":
      return "Continue reading";
    case "finished":
      return "Read again";
  }
}

/**
 * Pick the next issue a user should read in a series, plus the matching
 * primary-button label. Algorithm (spec):
 *
 *   1. If any issue has an in-progress (page > 0, !finished) record, the
 *      most-recently-updated one is the resume target → "Continue reading".
 *   2. Else, the first issue in the natural sort order whose progress
 *      row is missing or marks it not-finished → "Read".
 *   3. Else (every issue is finished) → "Read again", starting from the
 *      first issue in the series.
 *   4. If the series has no active issues at all → null target.
 *
 * Issues are expected pre-sorted by sort number. Soft-deleted / encrypted
 * issues are filtered out — the user can't read them, so they shouldn't
 * become the resume target.
 */
export function pickNextIssue(
  issues: IssueSummaryView[],
  progressByIssueId: Map<string, ProgressLike>,
): { target: IssueSummaryView | null; state: ReadState } {
  const active = issues.filter((i) => i.state === "active");
  if (active.length === 0) {
    return { target: null, state: "unread" };
  }

  // "Main" run = issues at or after #1 (or unnumbered ones we
  // can't classify). Preludes — #0, #1/2, FCBD specials and any
  // other `sort_number < 1` entry — are usually opt-in extras the
  // user seeks out later; they shouldn't anchor the Read CTA's
  // starting point. Same partition the server's series-cover-pick
  // uses to anchor on issue #1 (m20261225 / hydrate_series). The
  // preludes still appear in the issue listing in their natural
  // order — only the auto-pick changes.
  const isMain = (i: IssueSummaryView) =>
    i.sort_number == null || i.sort_number >= 1;
  const main = active.filter(isMain);
  const isUnread = (i: IssueSummaryView) => {
    const p = progressByIssueId.get(i.id);
    return !p || !p.finished;
  };

  // 1. Most-recently-updated in-progress issue. Preludes count here —
  // if the user opened #1/2 deliberately, the resume should take them
  // back there.
  let bestInProgress: { issue: IssueSummaryView; updatedAt: string } | null =
    null;
  for (const issue of active) {
    const p = progressByIssueId.get(issue.id);
    if (!p) continue;
    if (p.finished) continue;
    if (p.page <= 0) continue;
    const updatedAt = p.updated_at ?? "";
    if (
      !bestInProgress ||
      updatedAt.localeCompare(bestInProgress.updatedAt) > 0
    ) {
      bestInProgress = { issue, updatedAt };
    }
  }
  if (bestInProgress) {
    return { target: bestInProgress.issue, state: "in_progress" };
  }

  // 2. First not-finished MAIN issue. Skips preludes so a series
  // with #1/2 + #1 + … gets a Read button that lands on #1.
  const firstMainUnread = main.find(isUnread);
  if (firstMainUnread) {
    return { target: firstMainUnread, state: "unread" };
  }

  // 2b. Every main issue is finished. Fall back to any unread prelude
  // so the user can mop up the specials before the series is truly
  // done.
  const firstPreludeUnread = active.find(isUnread);
  if (firstPreludeUnread) {
    return { target: firstPreludeUnread, state: "unread" };
  }

  // 3. Every active issue is finished — restart from the canonical
  // start (#1) when one exists, otherwise from whatever comes first.
  return { target: main[0] ?? active[0] ?? null, state: "finished" };
}

/** Build a `Map<issue_id, ProgressLike>` from a `/progress` delta payload. */
export function indexProgress(
  records: ProgressLike[],
  filter?: Set<string>,
): Map<string, ProgressLike> {
  const out = new Map<string, ProgressLike>();
  for (const r of records) {
    if (filter && !filter.has(r.issue_id)) continue;
    out.set(r.issue_id, r);
  }
  return out;
}
