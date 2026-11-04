/** Read progress around finishing an issue — of the series, or of the
 *  reading list (CBL) the issue was opened from. Counts are issues. */
export type SeriesFinishProgress = {
  /** What the ring measures: the series, or the reading list. */
  scope: "series" | "list";
  /** The series' or the list's name. */
  seriesName: string;
  total: number;
  /** Finished issues before this one was finished. */
  before: number;
  /** Finished issues now. */
  after: number;
};

/**
 * Pure: the series progress the end card shows, from the series' finished
 * count as of opening the reader. `counts` is false when this read does
 * not record progress (the issue was already finished, incognito, peek):
 * then nothing moves.
 */
export function seriesFinishProgress(
  seriesName: string,
  snapshot: { finished: number; total: number } | null | undefined,
  counts: boolean,
  scope: "series" | "list" = "series",
): SeriesFinishProgress | null {
  if (!snapshot || snapshot.total <= 0) return null;
  const total = snapshot.total;
  const before = Math.max(0, Math.min(total, snapshot.finished));
  const after = counts ? Math.min(total, before + 1) : before;
  return { scope, seriesName, total, before, after };
}
