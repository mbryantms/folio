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

  // 1. Most-recently-updated in-progress issue.
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

  // 2. First not-finished issue (no record OR record with page=0).
  const firstUnread = active.find((i) => {
    const p = progressByIssueId.get(i.id);
    return !p || !p.finished;
  });
  if (firstUnread) {
    return { target: firstUnread, state: "unread" };
  }

  // 3. Every active issue is finished — restart from the top.
  return { target: active[0] ?? null, state: "finished" };
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
