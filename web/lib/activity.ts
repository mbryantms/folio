/**
 * Pure helpers shared by the M6a `/settings/activity` page and (in M6b) the
 * series + issue Activity tabs. Kept separate so the formatting + grouping
 * logic is unit-testable without rendering a React tree.
 */

import type { ReadingSessionView } from "@/lib/api/types";

/**
 * Group a flat list of sessions by their local-date prefix (`YYYY-MM-DD`
 * from the RFC3339 timestamp). The server is expected to return sessions
 * sorted DESC by `started_at`; this preserves that order within each
 * day-bucket so the UI sees newest-on-top.
 *
 * Note: this uses the date prefix of the *server-emitted* timestamp, which
 * is UTC. M6b's heatmap will use the user's timezone via the server's
 * stats endpoint — for the timeline view, UTC-derived bucketing is fine
 * since the row also shows the local time.
 */
export function groupSessionsByDay(
  records: ReadonlyArray<ReadingSessionView>,
): Map<string, ReadingSessionView[]> {
  const byDay = new Map<string, ReadingSessionView[]>();
  for (const r of records) {
    const day = r.started_at.slice(0, 10);
    const arr = byDay.get(day) ?? [];
    arr.push(r);
    byDay.set(day, arr);
  }
  return byDay;
}

/** `'2026-05-06'` → `'Today'` / `'Yesterday'` / `'Sunday, May 4'`. */
export function formatDayLabel(iso: string, today: Date = new Date()): string {
  const todayIso = toIsoDate(today);
  if (iso === todayIso) return "Today";
  const y = new Date(today);
  y.setDate(y.getDate() - 1);
  if (iso === toIsoDate(y)) return "Yesterday";
  return new Date(`${iso}T00:00:00`).toLocaleDateString(undefined, {
    weekday: "long",
    month: "short",
    day: "numeric",
  });
}

function toIsoDate(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function formatTimeOfDay(iso: string): string {
  return new Date(iso).toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
}

/**
 * Active-time formatter. Picks the right unit ladder so 45s reads as
 * `45s`, 5 min reads as `5m 0s`, 90 min reads as `1h 30m`.
 */
export function formatDurationMs(ms: number): string {
  const totalSec = Math.max(0, Math.round(ms / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

/** Hours formatter for the totals card — 0.5h → `30m`, 1.7h → `1.7h`. */
export function formatTotalHours(hours: number): string {
  if (hours >= 1) return `${hours.toFixed(1)}h`;
  const minutes = Math.round(hours * 60);
  return `${minutes}m`;
}

/**
 * Render a reading-session row as a human-readable label. Falls back
 * progressively when joined data is missing, so the timeline never has
 * to show a raw BLAKE3 hash. Format ladder:
 *   Series #N · Title   (number + title)
 *   Series #N           (number, no title)
 *   Series — Title      (title, no number)
 *   Series              (series only)
 *   <hash 12-char>…     (last-resort fallback)
 */
export function labelForSession(session: {
  issue_id: string;
  issue_title?: string | null;
  issue_number?: string | null;
  series_name?: string | null;
}): string {
  const series = session.series_name?.trim() || "";
  const number = session.issue_number?.trim() || "";
  const title = session.issue_title?.trim() || "";

  if (series && number && title) return `${series} #${number} · ${title}`;
  if (series && number) return `${series} #${number}`;
  if (series && title) return `${series} — ${title}`;
  if (series) return series;
  if (title) return title;
  return `${session.issue_id.slice(0, 12)}…`;
}

/** Build the points-string for an inline-SVG sparkline. Exposed so the
 *  render math is unit-testable. */
export function sparklinePoints(
  series: ReadonlyArray<number>,
  width: number,
  height: number,
  pad = 2,
): string {
  if (series.length === 0) return "";
  const max = Math.max(1, ...series);
  const step = series.length > 1 ? (width - pad * 2) / (series.length - 1) : 0;
  return series
    .map((v, i) => {
      const x = pad + i * step;
      const y = height - pad - (v / max) * (height - pad * 2);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

// ────────────── Heatmap (M6b) ──────────────

export type HeatmapCell = {
  /** `YYYY-MM-DD`. */
  date: string;
  /** Raw value (active_ms in our case). 0 means "no activity that day". */
  value: number;
  /** Discrete intensity bucket so the SVG can map to a small palette
   *  (0 = empty, 4 = max). */
  intensity: 0 | 1 | 2 | 3 | 4;
  /** False for cells before the user's first read or after today — these
   *  are rendered translucent so the year-back rectangle still reads as a
   *  full grid. */
  inRange: boolean;
};

export type HeatmapGrid = {
  /** 53 columns × 7 rows. `cells[col][row]` where row 0 = Sunday. */
  cells: HeatmapCell[][];
  /** First-of-month labels keyed to their column index for SVG `<text>`
   *  placement. */
  monthLabels: ReadonlyArray<{ col: number; label: string }>;
  /** The maximum value seen — exposed so the legend / tooltip can scale. */
  max: number;
};

/**
 * Build a year-back GitHub-style activity grid. Rightmost column ends with
 * `today`; leftmost column is 52 weeks earlier (so 53 columns total).
 * Each column starts on Sunday — empty cells before the first day-with-
 * activity and after today are flagged `inRange: false` so the renderer
 * can dim them.
 *
 * `perDay` is the server's bucketed `per_day` array; values are summed
 * by `YYYY-MM-DD` so duplicate keys (shouldn't happen but cheap) are
 * tolerated.
 */
export function buildHeatmapGrid(
  perDay: ReadonlyArray<{ date: string; active_ms: number }>,
  today: Date = new Date(),
): HeatmapGrid {
  const byDate = new Map<string, number>();
  for (const d of perDay) {
    byDate.set(d.date, (byDate.get(d.date) ?? 0) + d.active_ms);
  }
  const max = Math.max(0, ...byDate.values());

  // Anchor: rightmost column = the week containing `today`. Slide back
  // to that week's Sunday so each column reads top-to-bottom Sun..Sat.
  const todayMid = startOfDay(today);
  const dow = todayMid.getDay(); // 0..6, Sun = 0
  const sundayThisWeek = addDays(todayMid, -dow);
  const startSunday = addDays(sundayThisWeek, -52 * 7);

  const cells: HeatmapCell[][] = [];
  const monthLabels: { col: number; label: string }[] = [];
  let lastMonth = -1;

  for (let col = 0; col < 53; col += 1) {
    const colCells: HeatmapCell[] = [];
    for (let row = 0; row < 7; row += 1) {
      const d = addDays(startSunday, col * 7 + row);
      const iso = toIsoDate(d);
      const value = byDate.get(iso) ?? 0;
      const inRange = d <= todayMid;
      colCells.push({
        date: iso,
        value,
        intensity: bucketIntensity(value, max),
        inRange,
      });
    }
    // Label a column with its month name when it's the first column whose
    // first day belongs to a new month — cheap heuristic that matches
    // GitHub's layout.
    const firstDay = addDays(startSunday, col * 7);
    if (firstDay.getMonth() !== lastMonth) {
      monthLabels.push({
        col,
        label: firstDay.toLocaleDateString(undefined, { month: "short" }),
      });
      lastMonth = firstDay.getMonth();
    }
    cells.push(colCells);
  }
  return { cells, monthLabels, max };
}

export function bucketIntensity(value: number, max: number): 0 | 1 | 2 | 3 | 4 {
  if (value <= 0 || max <= 0) return 0;
  const ratio = value / max;
  if (ratio < 0.25) return 1;
  if (ratio < 0.5) return 2;
  if (ratio < 0.75) return 3;
  return 4;
}

/**
 * DOW × hour heatmap uses the same intensity steps as the year heatmap but
 * with peak-anchored thresholds so a single hot cell doesn't crush the rest
 * of the grid into the 0 bucket.
 */
export function dowHourBucket(value: number, max: number): 0 | 1 | 2 | 3 | 4 {
  if (value <= 0 || max <= 0) return 0;
  const ratio = value / max;
  if (ratio >= 0.75) return 4;
  if (ratio >= 0.5) return 3;
  if (ratio >= 0.25) return 2;
  return 1;
}

/**
 * Trailing moving average over `window` samples ending at `end`. Used by
 * the pace chart to overlay a smoothed trend on the per-session points.
 * `samples` is expected to be sorted ascending by time; we don't enforce
 * that here since the caller controls the data shape.
 */
export function movingAverage(
  samples: ReadonlyArray<number>,
  end: number,
  window: number,
): number {
  if (samples.length === 0 || end < 0) return 0;
  const last = Math.min(end, samples.length - 1);
  const start = Math.max(0, last - window + 1);
  let acc = 0;
  let n = 0;
  for (let i = start; i <= last; i += 1) {
    acc += samples[i]!;
    n += 1;
  }
  return n > 0 ? acc / n : 0;
}

/**
 * Roll a sparse DOW × hour grid into 4 time-of-day buckets:
 * morning 05-11, afternoon 12-16, evening 17-21, night 22-04. Returns the
 * percentage of total active_ms in each bucket plus the absolute ms.
 *
 * Mirrors the server's roll (see `time_of_day_from`) so client-side
 * derivations stay consistent if the server payload is unavailable.
 */
export function timeOfDayBuckets(
  cells: ReadonlyArray<{ hour: number; active_ms: number }>,
): { morning: number; afternoon: number; evening: number; night: number } {
  const buckets = { morning: 0, afternoon: 0, evening: 0, night: 0 };
  for (const c of cells) {
    if (c.hour >= 5 && c.hour <= 11) buckets.morning += c.active_ms;
    else if (c.hour >= 12 && c.hour <= 16) buckets.afternoon += c.active_ms;
    else if (c.hour >= 17 && c.hour <= 21) buckets.evening += c.active_ms;
    else buckets.night += c.active_ms;
  }
  return buckets;
}

function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}
function addDays(d: Date, n: number): Date {
  const out = new Date(d);
  out.setDate(d.getDate() + n);
  return out;
}
