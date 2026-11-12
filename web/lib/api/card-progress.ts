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
 * A write made in this tab (mark, bulk mark, Undo, the reader's page
 * writes) is shown from the progress overlay until a list fetched after
 * it supersedes it — see [`progress-overlay.ts`](./progress-overlay.ts)
 * for the ordering rule (by when this tab saw each copy, never by clock).
 */
import { useQueryClient } from "@tanstack/react-query";

import { useOverlayEntry, type OverlayRecord } from "./progress-overlay";
import { useUserProgress } from "./queries";
import type { IssueSummaryView } from "./types";

/** The fields a badge needs, from any of the progress shapes. */
export type CardProgressRecord = OverlayRecord;

/** The viewer's progress on `issue` for its card badge; `null` = unread
 *  (or still loading on the fallback path, which draws no badge either). */
export function useCardProgress(
  issue: Pick<IssueSummaryView, "id" | "progress"> | undefined,
): CardProgressRecord | null {
  const qc = useQueryClient();
  const id = issue?.id ?? "";
  const embedded = issue?.progress;
  const entry = useOverlayEntry(qc, id);
  const needsMap = issue !== undefined && embedded === undefined && !entry;
  const map = useUserProgress({ enabled: needsMap }).data;
  if (!issue) return null;
  if (entry) return entry.record;
  return (embedded === undefined ? map?.get(id) : embedded) ?? null;
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
