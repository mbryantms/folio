/**
 * The read check / progress bar an issue card draws.
 *
 * Cards read the viewer's progress from the card itself
 * (`IssueSummaryView.progress`, embedded by every issue-card list the
 * server serves): `null` = no record, an object = the record. Marks patch
 * that copy in the query cache ([`card-list-cache.ts`](./card-list-cache.ts)),
 * so the card has nothing else to consult. A card from a response without
 * the field (`undefined`) falls back to the shared all-progress map
 * (`useUserProgress`), which is only requested then.
 */
import { useUserProgress } from "./queries";
import type { IssueCardProgress, IssueSummaryView } from "./types";

/** The fields a badge needs, from any of the progress shapes. */
export type CardProgressRecord = Pick<
  IssueCardProgress,
  "finished" | "percent"
>;

/** The viewer's progress on `issue` for its card badge; `null` = unread
 *  (or still loading on the fallback path, which draws no badge either). */
export function useCardProgress(
  issue: Pick<IssueSummaryView, "id" | "progress"> | undefined,
): CardProgressRecord | null {
  const embedded = issue?.progress;
  const needsMap = issue !== undefined && embedded === undefined;
  const map = useUserProgress({ enabled: needsMap }).data;
  if (!issue) return null;
  return (needsMap ? map?.get(issue.id) : embedded) ?? null;
}

/** Badge state for a card: finished check, or a bar at `percent` (0–100). */
export function cardBadge(p: CardProgressRecord | null): {
  finished: boolean;
  inProgress: boolean;
  percent: number;
} {
  const finished = p?.finished ?? false;
  const inProgress = !!p && !finished && p.percent > 0;
  const percent = inProgress
    ? Math.max(0, Math.min(100, Math.round(p.percent * 100)))
    : 0;
  return { finished, inProgress, percent };
}
