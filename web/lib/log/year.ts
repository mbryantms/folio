/**
 * Pure maths behind the reading log's Year in review story
 * (`/log/year/<year>`, folio-flair phase 4): the calendar-year window,
 * monthly bars, the year's longest run, when you read, and the covers of
 * what you finished. Inputs are the stats endpoint's year-scoped buckets
 * (`range=YYYY`, dates `YYYY-MM-DD` in the user's timezone) and reading-log
 * events. Plan: ~/.claude/plans/folio-flair-phase4-log.md
 */

import {
  dayLabel,
  localDate,
  monthShort,
  readingYears,
  streakRuns,
  type DayPages,
  type Run,
} from "./overview";

/** A four-digit year from a route segment, or null. */
export function parseYear(raw: string): number | null {
  if (!/^\d{4}$/.test(raw)) return null;
  const y = +raw;
  return y >= 1970 && y <= 9999 ? y : null;
}

/**
 * RFC3339 bounds for fetching the year's reading-log events. The log takes
 * instants, the year is in the user's timezone, so the window is padded a
 * day on each side; callers keep only events whose local date is in the
 * year (`inYear`).
 */
export function yearFetchWindow(year: number): { from: string; to: string } {
  return {
    from: `${year - 1}-12-31T00:00:00Z`,
    to: `${year + 1}-01-02T00:00:00Z`,
  };
}

/** Whether an instant falls in `year` in `timeZone`. */
export function inYear(at: string, year: number, timeZone: string): boolean {
  return localDate(new Date(at), timeZone).startsWith(`${year}-`);
}

/** Days in a calendar year. */
export function daysInYear(year: number): number {
  return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0 ? 366 : 365;
}

export type MonthBar = {
  /** `YYYY-MM`. */
  key: string;
  /** "Jan". */
  label: string;
  pages: number;
};

/** Twelve monthly page totals for `year` (months without reading = 0). */
export function monthBars(days: readonly DayPages[], year: number): MonthBar[] {
  const bars: MonthBar[] = Array.from({ length: 12 }, (_, i) => {
    const key = `${year}-${String(i + 1).padStart(2, "0")}`;
    return { key, label: monthShort(`${key}-01`), pages: 0 };
  });
  for (const d of days) {
    if (!d.date.startsWith(`${year}-`)) continue;
    const bar = bars[+d.date.slice(5, 7) - 1];
    if (bar) bar.pages += d.pages;
  }
  return bars;
}

/** Index of the month with the most pages (earliest wins a tie), or -1. */
export function busiestMonth(bars: readonly MonthBar[]): number {
  let best = -1;
  bars.forEach((b, i) => {
    if (b.pages > 0 && (best < 0 || b.pages > bars[best]!.pages)) best = i;
  });
  return best;
}

/** The day with the most pages (earliest wins a tie). */
export function biggestDay(days: readonly DayPages[]): DayPages | null {
  let best: DayPages | null = null;
  for (const d of [...days].sort((a, b) => a.date.localeCompare(b.date)))
    if (d.pages > 0 && (!best || d.pages > best.pages)) best = d;
  return best;
}

/** The longest run of consecutive reading days inside `year`. */
export function yearLongestRun(
  days: readonly DayPages[],
  year: number,
): Run | null {
  // `today` past the year so no run counts as "current".
  return streakRuns(
    days.filter((d) => d.date.startsWith(`${year}-`)),
    `${year + 1}-12-31`,
  ).longest;
}

/** "Tue 3 Nov – Sat 14 Nov" (one label for a one-day run). */
export function runLabel(run: Run): string {
  const y = run.start.slice(0, 4);
  return run.start === run.end
    ? dayLabel(run.start, y)
    : `${dayLabel(run.start, y)} – ${dayLabel(run.end, y)}`;
}

export type DowHour = {
  dow: number;
  hour: number;
  active_ms: number;
  sessions: number;
};

export const WEEKDAYS = [
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
  "Sunday",
] as const;

export type TimeOfDay = "morning" | "afternoon" | "evening" | "night";

/** The server's buckets: 05–11 morning, 12–16 afternoon, 17–21 evening,
 *  22–04 night. */
export function timeOfDay(hour: number): TimeOfDay {
  if (hour >= 5 && hour < 12) return "morning";
  if (hour >= 12 && hour < 17) return "afternoon";
  if (hour >= 17 && hour < 22) return "evening";
  return "night";
}

export const TIME_OF_DAY: readonly {
  id: TimeOfDay;
  label: string;
  persona: string;
}[] = [
  { id: "morning", label: "Morning", persona: "an early bird" },
  { id: "afternoon", label: "Afternoon", persona: "an afternoon reader" },
  { id: "evening", label: "Evening", persona: "an evening reader" },
  { id: "night", label: "Night", persona: "a night owl" },
];

export type Rhythm = {
  /** Reading time per weekday, Monday first. */
  byDow: number[];
  /** Share of reading time per time of day, 0–1, in `TIME_OF_DAY` order. */
  byTime: { id: TimeOfDay; label: string; share: number }[];
  /** Index into `WEEKDAYS`. */
  peakDow: number;
  peakHour: number;
  peakTime: TimeOfDay;
  persona: string;
};

/**
 * When you read, from the sparse 7×24 grid (server `dow` is 0 = Sunday).
 * Weighted by reading time, falling back to session counts for grids with
 * no time. Null when there is nothing to say.
 */
export function readingRhythm(cells: readonly DowHour[]): Rhythm | null {
  const weight = cells.some((c) => c.active_ms > 0)
    ? (c: DowHour) => c.active_ms
    : (c: DowHour) => c.sessions;
  const byDow = Array<number>(7).fill(0);
  const byHour = Array<number>(24).fill(0);
  for (const c of cells) {
    const w = Math.max(0, weight(c));
    byDow[(c.dow + 6) % 7]! += w;
    byHour[((c.hour % 24) + 24) % 24]! += w;
  }
  const total = byDow.reduce((s, x) => s + x, 0);
  if (total <= 0) return null;
  const argmax = (xs: number[]) =>
    xs.reduce((best, x, i) => (x > xs[best]! ? i : best), 0);
  const tod = new Map<TimeOfDay, number>();
  byHour.forEach((w, h) =>
    tod.set(timeOfDay(h), (tod.get(timeOfDay(h)) ?? 0) + w),
  );
  const byTime = TIME_OF_DAY.map((t) => ({
    id: t.id,
    label: t.label,
    share: (tod.get(t.id) ?? 0) / total,
  }));
  const peakTime = byTime.reduce((b, t) => (t.share > b.share ? t : b)).id;
  const peakHour = argmax(byHour);
  return {
    byDow,
    byTime,
    peakDow: argmax(byDow),
    peakHour,
    peakTime,
    persona: TIME_OF_DAY.find((t) => t.id === peakTime)!.persona,
  };
}

/** "9 pm", "12 am", "noon". */
export function hourLabel(hour: number): string {
  if (hour === 12) return "noon";
  if (hour === 0) return "midnight";
  return `${hour % 12} ${hour < 12 ? "am" : "pm"}`;
}

/** Whole hours, or minutes below an hour: "42 hours", "35 minutes". */
export function durationParts(ms: number): { value: number; unit: string } {
  const hours = ms / 3_600_000;
  if (hours >= 1) {
    const h = Math.round(hours);
    return { value: h, unit: h === 1 ? "hour" : "hours" };
  }
  const m = Math.round(ms / 60_000);
  return { value: m, unit: m === 1 ? "minute" : "minutes" };
}

/** A finished issue from the reading log, reduced to what the story uses. */
export type FinishedCover = {
  /** Event id, for dedupe across pages. */
  id: string;
  issueId: string | null;
  seriesId: string | null;
  seriesName: string;
  cover: string | null;
  seriesCover: string | null;
};

/** Distinct issue covers, newest first as given, capped at `max`. */
export function mosaicCovers(
  finished: readonly FinishedCover[],
  max = 60,
): { covers: { key: string; src: string; title: string }[]; total: number } {
  const seen = new Set<string>();
  const covers: { key: string; src: string; title: string }[] = [];
  let total = 0;
  for (const f of finished) {
    const key = f.issueId ?? f.id;
    if (seen.has(key)) continue;
    seen.add(key);
    total += 1;
    const src = f.cover ?? f.seriesCover;
    if (src && covers.length < max)
      covers.push({ key, src, title: f.seriesName });
  }
  return { covers, total };
}

export type TopSeries = {
  series_id: string;
  name: string;
  active_ms: number;
  sessions: number;
};

/**
 * The top series with a cover each: the series cover from any of its
 * finished issues' events, else its most recently finished issue's cover.
 */
export function topSeriesWithCovers(
  top: readonly TopSeries[],
  finished: readonly FinishedCover[],
  max = 5,
): (TopSeries & { cover: string | null; finished: number })[] {
  const seriesCovers = new Map<string, string>();
  const issueCovers = new Map<string, string>();
  const counts = new Map<string, number>();
  for (const f of finished) {
    if (!f.seriesId) continue;
    counts.set(f.seriesId, (counts.get(f.seriesId) ?? 0) + 1);
    if (f.seriesCover && !seriesCovers.has(f.seriesId))
      seriesCovers.set(f.seriesId, f.seriesCover);
    if (f.cover && !issueCovers.has(f.seriesId))
      issueCovers.set(f.seriesId, f.cover);
  }
  return top.slice(0, max).map((s) => ({
    ...s,
    cover:
      seriesCovers.get(s.series_id) ?? issueCovers.get(s.series_id) ?? null,
    finished: counts.get(s.series_id) ?? 0,
  }));
}

/**
 * Years offered by the picker: every year with reading (all-time
 * buckets), plus the one being shown, newest first.
 */
export function pickerYears(
  allDays: readonly DayPages[],
  shown: number,
): number[] {
  return [...new Set([...readingYears(allDays), shown])].sort((a, b) => b - a);
}

export type SlideId =
  | "intro"
  | "numbers"
  | "months"
  | "top"
  | "rhythm"
  | "streak"
  | "mosaic"
  | "outro"
  | "empty";

export const SLIDE_TITLES: Record<SlideId, string> = {
  intro: "Your year",
  numbers: "Pages and time",
  months: "Busiest month",
  top: "Top series",
  rhythm: "When you read",
  streak: "Longest streak",
  mosaic: "Everything you finished",
  outro: "Summary",
  empty: "No reading",
};

/**
 * The story's slides for a year: an empty year gets one slide; otherwise
 * intro → numbers → months → top series → rhythm → streak → mosaic →
 * outro, skipping any the data can't fill.
 */
export function storySlides(m: {
  pages: number;
  sessions: number;
  daysActive: number;
  busiest: number;
  topCount: number;
  hasRhythm: boolean;
  run: Run | null;
  finished: number;
}): SlideId[] {
  if (m.sessions <= 0 && m.pages <= 0 && m.daysActive <= 0) return ["empty"];
  const out: SlideId[] = ["intro", "numbers"];
  if (m.busiest >= 0) out.push("months");
  if (m.topCount > 0) out.push("top");
  if (m.hasRhythm) out.push("rhythm");
  if (m.run && m.run.length >= 2) out.push("streak");
  if (m.finished > 0) out.push("mosaic");
  out.push("outro");
  return out;
}
