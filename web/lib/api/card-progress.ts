/**
 * The read check / progress bar an issue card draws.
 *
 * Cards read the viewer's progress from the card itself
 * (`IssueSummaryView.progress`, embedded by every issue-card list the
 * server serves): `null` = no record, an object = the record. A card
 * from a response without the field (`undefined`) falls back to the
 * shared all-progress map (`useUserProgress`), which is only requested
 * then — so a page whose lists all embed progress never asks for every
 * record of the user.
 *
 * Writes made in this tab land under `queryKeys.issueProgress(id)` (the
 * optimistic mark, the mark's reply, the reader's per-page reply); the
 * card shows whichever of that and its list copy was written last, so a
 * badge follows a read without refetching the list it came from.
 */
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useUserProgress } from "./queries";
import { queryKeys } from "./query-keys";
import type { IssueSummaryView } from "./types";

/** The fields a badge needs, from any of the progress shapes. */
export type CardProgressRecord = {
  finished: boolean;
  percent: number;
  updated_at: string;
};

function writtenAt(r: CardProgressRecord): number {
  const t = Date.parse(r.updated_at);
  return Number.isNaN(t) ? Number.NEGATIVE_INFINITY : t;
}

/** `written` when it is newer than `listed` (or the list has none). */
export function latestCardProgress(
  listed: CardProgressRecord | null | undefined,
  written: CardProgressRecord | null | undefined,
): CardProgressRecord | null {
  if (!written) return listed ?? null;
  if (!listed) return written;
  return writtenAt(written) > writtenAt(listed) ? written : listed;
}

/** The viewer's progress on `issue` for its card badge; `null` = unread
 *  (or still loading on the fallback path, which draws no badge either). */
export function useCardProgress(
  issue: Pick<IssueSummaryView, "id" | "progress"> | undefined,
): CardProgressRecord | null {
  const qc = useQueryClient();
  const id = issue?.id ?? "";
  const embedded = issue?.progress;
  const needsMap = issue !== undefined && embedded === undefined;
  const map = useUserProgress({ enabled: needsMap }).data;
  // Observes the cache only — never fetches (same as the issue page's
  // `useIssueReadState`).
  const { data: written } = useQuery({
    queryKey: queryKeys.issueProgress(id),
    queryFn: () =>
      qc.getQueryData<CardProgressRecord | null>(queryKeys.issueProgress(id)) ??
      null,
    enabled: false,
  });
  if (!issue) return null;
  const listed = needsMap ? map?.get(id) : embedded;
  return latestCardProgress(listed, written);
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
