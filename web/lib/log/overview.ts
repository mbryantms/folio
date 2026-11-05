/**
 * Pure maths behind the reading log's Overview tab (folio-flair phase 4):
 * streak runs, the activity heatmap grid and milestones, all from the
 * stats endpoint's `per_day` buckets (dates are `YYYY-MM-DD` in the
 * user's timezone). Plan: ~/.claude/plans/folio-flair-phase4-log.md
 */

export type DayPages = { date: string; pages: number };

/** `YYYY-MM-DD` for `d` in `timeZone` (falls back to UTC on a bad zone). */
export function localDate(d: Date, timeZone: string): string {
  try {
    return new Intl.DateTimeFormat("en-CA", {
      timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    }).format(d);
  } catch {
    return d.toISOString().slice(0, 10);
  }
}

/** Whole days from `a` to `b` (`YYYY-MM-DD`, calendar days, DST-proof). */
export function dayDiff(a: string, b: string): number {
  const ms =
    Date.UTC(+b.slice(0, 4), +b.slice(5, 7) - 1, +b.slice(8, 10)) -
    Date.UTC(+a.slice(0, 4), +a.slice(5, 7) - 1, +a.slice(8, 10));
  return Math.round(ms / 86_400_000);
}

/** `date` shifted by `n` days (`YYYY-MM-DD`). */
export function addDays(date: string, n: number): string {
  const t = Date.UTC(
    +date.slice(0, 4),
    +date.slice(5, 7) - 1,
    +date.slice(8, 10),
  );
  return new Date(t + n * 86_400_000).toISOString().slice(0, 10);
}

export type Run = { start: string; end: string; length: number };

/**
 * Runs of consecutive reading days. `current` is the run ending today or
 * yesterday (a streak survives until a full day is missed, matching the
 * server's count); `longest` the longest ever (latest wins a tie).
 */
export function streakRuns(
  days: readonly DayPages[],
  today: string,
): { current: Run | null; longest: Run | null } {
  // Any day with a bucket counts as a reading day (the server counts
  // session days, pages or not).
  const dates = [...new Set(days.map((d) => d.date))].sort();
  let longest: Run | null = null;
  let run: Run | null = null;
  for (const date of dates) {
    if (run && dayDiff(run.end, date) === 1) {
      run = { start: run.start, end: date, length: run.length + 1 };
    } else {
      run = { start: date, end: date, length: 1 };
    }
    if (!longest || run.length >= longest.length) longest = run;
  }
  const current = run && dayDiff(run.end, today) <= 1 ? run : null;
  return { current, longest };
}

/** Heat level 0–4 for a day's pages. */
export function heatLevel(pages: number): 0 | 1 | 2 | 3 | 4 {
  if (pages <= 0) return 0;
  if (pages <= 20) return 1;
  if (pages <= 45) return 2;
  if (pages <= 80) return 3;
  return 4;
}

export type HeatCell = {
  date: string;
  pages: number;
  level: 0 | 1 | 2 | 3 | 4;
  /** Part of the current streak (drawn outlined). */
  streak: boolean;
  /** After today: drawn empty and not interactive. */
  future: boolean;
};

/**
 * The heatmap grid: `weeks` columns of Monday-first days ending with the
 * week containing `today`, plus a month label at the first column of
 * each month (labels closer than 3 columns apart are dropped).
 */
export function heatmapGrid(
  days: readonly DayPages[],
  today: string,
  current: Run | null,
  weeks = 53,
): { columns: HeatCell[][]; months: { column: number; label: string }[] } {
  const pages = new Map(days.map((d) => [d.date, d.pages]));
  // Monday of today's week.
  const dow = (new Date(`${today}T00:00:00Z`).getUTCDay() + 6) % 7;
  const firstMonday = addDays(today, -dow - (weeks - 1) * 7);
  const columns: HeatCell[][] = [];
  const months: { column: number; label: string }[] = [];
  let lastLabelCol = -9;
  for (let w = 0; w < weeks; w++) {
    const col: HeatCell[] = [];
    for (let d = 0; d < 7; d++) {
      const date = addDays(firstMonday, w * 7 + d);
      const p = pages.get(date) ?? 0;
      col.push({
        date,
        pages: p,
        level: heatLevel(p),
        streak: !!current && date >= current.start && date <= current.end,
        future: date > today,
      });
      const dom = +date.slice(8, 10);
      if (dom === 1 || (w === 0 && d === 0 && dom <= 14)) {
        if (w - lastLabelCol >= 3) {
          months.push({ column: w, label: monthShort(date) });
          lastLabelCol = w;
        }
      }
    }
    columns.push(col);
  }
  return { columns, months };
}

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];
export function monthShort(date: string): string {
  return MONTHS[+date.slice(5, 7) - 1] ?? "";
}

/** "Tue 3 Nov" (or with the year when it isn't `thisYear`). */
export function dayLabel(date: string, thisYear?: string): string {
  const d = new Date(`${date}T00:00:00Z`);
  const wd = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][d.getUTCDay()];
  const base = `${wd} ${+date.slice(8, 10)} ${monthShort(date)}`;
  return thisYear && date.slice(0, 4) !== thisYear
    ? `${base} ${date.slice(0, 4)}`
    : base;
}

/** Pages per calendar month (`YYYY-MM`). */
export function monthlyPages(days: readonly DayPages[]): Map<string, number> {
  const m = new Map<string, number>();
  for (const d of days)
    m.set(d.date.slice(0, 7), (m.get(d.date.slice(0, 7)) ?? 0) + d.pages);
  return m;
}

/** Years with any reading, newest first. */
export function readingYears(days: readonly DayPages[]): number[] {
  return [
    ...new Set(days.filter((d) => d.pages > 0).map((d) => +d.date.slice(0, 4))),
  ].sort((a, b) => b - a);
}

export type MilestoneIcon = "flame" | "check" | "book" | "star";

export type Milestone = {
  id: string;
  icon: MilestoneIcon;
  title: string;
  sub: string;
  earned: boolean;
  /** Locked only: progress toward it, 0–1. */
  progress?: number;
  /** Earned: when it was reached, for ordering (`YYYY-MM-DD`). */
  at?: string;
  /** Earned: a date window whose reading illustrates it (replay covers). */
  window?: { from: string; to: string };
  /** Earned series finish: the series it was. */
  seriesId?: string;
};

export const STREAK_TIERS = [7, 14, 30, 100] as const;
export const YEAR_PAGE_TIERS = [1_000, 5_000, 10_000, 25_000] as const;
export const MONTH_PAGES = 1_000;

const fmt = (n: number) => n.toLocaleString("en-US");

/**
 * Milestones: streak tiers (from the longest run), pages read in the
 * year, a 1,000-page month, and finished series. Earned ones first
 * (newest first), then the next locked tier of each ladder with its
 * progress.
 */
export function milestones(input: {
  days: readonly DayPages[];
  today: string;
  year: number;
  seriesFinished: readonly {
    seriesId: string;
    name: string;
    totalIssues: number;
    at: string;
  }[];
}): Milestone[] {
  const { days, today, year } = input;
  const earned: Milestone[] = [];
  const locked: Milestone[] = [];
  const { longest } = streakRuns(days, today);

  // Streaks: every tier the longest run reached, then the next one.
  const best = longest?.length ?? 0;
  for (const t of STREAK_TIERS) {
    if (longest && best >= t) {
      const reached = addDays(longest.start, t - 1);
      earned.push({
        id: `streak-${t}`,
        icon: "flame",
        title: `${t}-day streak`,
        sub: `Reached ${dayLabel(reached, today.slice(0, 4))}`,
        earned: true,
        at: reached,
        window: { from: longest.start, to: reached },
      });
    }
  }
  const nextStreak = STREAK_TIERS.find((t) => t > best);
  if (nextStreak) {
    locked.push({
      id: `streak-${nextStreak}`,
      icon: "flame",
      title: `${nextStreak}-day streak`,
      sub: best
        ? `Best so far: ${best} ${best === 1 ? "day" : "days"}`
        : "Read on consecutive days",
      earned: false,
      progress: best / nextStreak,
    });
  }

  // Pages read in the year.
  const yearDays = days
    .filter((d) => d.date.startsWith(`${year}-`))
    .sort((a, b) => a.date.localeCompare(b.date));
  const yearPages = yearDays.reduce((s, d) => s + d.pages, 0);
  let acc = 0;
  const reachedAt = new Map<number, string>();
  for (const d of yearDays) {
    acc += d.pages;
    for (const t of YEAR_PAGE_TIERS)
      if (acc >= t && !reachedAt.has(t)) reachedAt.set(t, d.date);
  }
  const topYearTier = [...YEAR_PAGE_TIERS]
    .reverse()
    .find((t) => reachedAt.has(t));
  if (topYearTier) {
    earned.push({
      id: `year-pages-${topYearTier}`,
      icon: "star",
      title: `${fmt(topYearTier)} pages in ${year}`,
      sub: `${fmt(yearPages)} pages so far`,
      earned: true,
      at: reachedAt.get(topYearTier),
      window: { from: `${year}-01-01`, to: reachedAt.get(topYearTier)! },
    });
  }
  const nextYearTier = YEAR_PAGE_TIERS.find((t) => t > yearPages);
  if (nextYearTier) {
    locked.push({
      id: `year-pages-${nextYearTier}`,
      icon: "star",
      title: `${fmt(nextYearTier)} pages in ${year}`,
      sub: `${fmt(yearPages)} so far`,
      earned: false,
      progress: yearPages / nextYearTier,
    });
  }

  // A 1,000-page month (the latest one; how many overall).
  const months = [...monthlyPages(days).entries()].sort((a, b) =>
    a[0].localeCompare(b[0]),
  );
  const big = months.filter(([, p]) => p >= MONTH_PAGES);
  const lastBig = big[big.length - 1];
  if (lastBig) {
    const [key, p] = lastBig;
    earned.push({
      id: "month-1000",
      icon: "book",
      title:
        big.length > 1 ? `1,000-page month ×${big.length}` : "1,000-page month",
      sub: `Latest: ${monthShort(`${key}-01`)} ${key.slice(0, 4)} · ${fmt(p)} pages`,
      earned: true,
      at: `${key}-28`,
      window: { from: `${key}-01`, to: addDays(`${key}-01`, 30) },
    });
  } else {
    const bestMonth = months.reduce<[string, number] | null>(
      (b, m) => (!b || m[1] > b[1] ? m : b),
      null,
    );
    locked.push({
      id: "month-1000",
      icon: "book",
      title: "1,000-page month",
      sub: bestMonth
        ? `Best: ${monthShort(`${bestMonth[0]}-01`)} ${bestMonth[0].slice(0, 4)} · ${fmt(bestMonth[1])} pages`
        : "Read 1,000 pages in a month",
      earned: false,
      progress: (bestMonth?.[1] ?? 0) / MONTH_PAGES,
    });
  }

  // Finished series (newest few).
  for (const s of input.seriesFinished.slice(0, 4)) {
    earned.push({
      id: `series-${s.seriesId}`,
      icon: "check",
      title: `Finished ${s.name}`,
      // Some finishes carry no issue count (older or bulk-marked rows).
      sub:
        s.totalIssues > 0
          ? `All ${s.totalIssues} ${s.totalIssues === 1 ? "issue" : "issues"} read`
          : "Every issue read",
      earned: true,
      at: s.at,
      seriesId: s.seriesId,
    });
  }

  earned.sort((a, b) => (b.at ?? "").localeCompare(a.at ?? ""));
  return [...earned, ...locked];
}
