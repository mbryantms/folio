import type { SeriesView } from "@/lib/api/types";

type Summary = NonNullable<SeriesView["progress_summary"]>;

/** What a series card shows about the viewer's reading of the series. */
export type SeriesReadState =
  | { kind: "unread"; label: string }
  | {
      kind: "started";
      label: string;
      /** Width of the card's progress bar, 0–100. */
      percent: number;
    }
  | { kind: "finished"; label: string };

/** A started series with nothing finished yet (only issues in progress)
 *  still gets a visible sliver, so it never looks untouched. */
export const MIN_STARTED_PERCENT = 5;

/**
 * Derive a series card's read state from the server's
 * `progress_summary` (the viewer's finished / in-progress counts over the
 * active issues). Returns `null` when there is nothing to say: no summary
 * on the payload, or a series with no active issues — the card then shows
 * no read state rather than guessing.
 *
 *  - finished: every active issue finished;
 *  - started:  at least one issue finished or in progress, but not all —
 *              the bar shows the share of issues finished;
 *  - unread:   nothing finished or in progress.
 *
 * `label` is the text a screen reader gets (and the tooltip).
 */
export function seriesReadState(
  summary: Summary | null | undefined,
): SeriesReadState | null {
  if (!summary) return null;
  const total = summary.total;
  if (!(total > 0)) return null;
  const finished = Math.min(Math.max(summary.finished, 0), total);
  const inProgress = Math.max(summary.in_progress, 0);
  if (finished >= total) {
    return {
      kind: "finished",
      label: total === 1 ? "Read" : `All ${total} issues read`,
    };
  }
  if (finished === 0 && inProgress === 0) {
    return { kind: "unread", label: "Unread" };
  }
  const percent = Math.max(
    MIN_STARTED_PERCENT,
    Math.round((finished / total) * 100),
  );
  const label =
    `${finished} of ${total} issue${total === 1 ? "" : "s"} read` +
    (inProgress > 0 ? `, ${inProgress} in progress` : "");
  return { kind: "started", label, percent };
}
