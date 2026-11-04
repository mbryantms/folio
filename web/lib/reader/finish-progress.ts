/** Series read progress around finishing an issue. Counts are issues. */
export type SeriesFinishProgress = {
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
): SeriesFinishProgress | null {
  if (!snapshot || snapshot.total <= 0) return null;
  const total = snapshot.total;
  const before = Math.max(0, Math.min(total, snapshot.finished));
  const after = counts ? Math.min(total, before + 1) : before;
  return { seriesName, total, before, after };
}
