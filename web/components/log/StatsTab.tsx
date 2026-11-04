"use client";

import { BarChart3 } from "lucide-react";
import dynamic from "next/dynamic";
import { useState } from "react";

import { useIsPhone } from "@/lib/ui/use-is-phone";

import {
  ActivityRangeSelector,
  parseRange,
} from "@/components/activity/ActivityRangeSelector";
import { DowHourHeatmap } from "@/components/activity/DowHourHeatmap";
import { HeroCards } from "@/components/activity/HeroCards";
import { RereadList } from "@/components/activity/RereadList";
import { TopRankingsList } from "@/components/activity/TopRankings";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import type {
  ReadingDayBucket,
  ReadingStatsRange,
  ReadingStatsView,
} from "@/lib/api/types";
import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";
import { useStickyToolbar } from "@/lib/ui/use-sticky-toolbar";
import { cn } from "@/lib/utils";
import { useReducedMotion } from "@/lib/visual-effects";

// Recharts is heavy and SSR-hostile; load it only once these charts are on
// screen (all three share one chunk).
const PerDayBarChart = dynamic(
  () =>
    import("@/components/activity/PerDayBarChart").then(
      (m) => m.PerDayBarChart,
    ),
  { ssr: false, loading: () => <Skeleton className="h-48 w-full" /> },
);
const PaceChart = dynamic(
  () => import("@/components/activity/PaceChart").then((m) => m.PaceChart),
  { ssr: false, loading: () => <Skeleton className="h-52 w-full" /> },
);
const TimeOfDayDonut = dynamic(
  () =>
    import("@/components/activity/TimeOfDayDonut").then(
      (m) => m.TimeOfDayDonut,
    ),
  { ssr: false, loading: () => <Skeleton className="h-48 w-full" /> },
);

const RANGE_CAPTION: Record<ReadingStatsRange, string> = {
  "7d": "Last 7 days",
  "30d": "Last 30 days",
  "60d": "Last 60 days",
  "90d": "Last 90 days",
  "1y": "Last 12 months",
  all: "All time",
};

const RANGE_DAYS: Partial<Record<ReadingStatsRange, number>> = {
  "7d": 7,
  "30d": 30,
  "60d": 60,
  "90d": 90,
  "1y": 365,
};

/** Above this many days the activity chart switches to one bar a week
 *  (sooner on a phone, where 90 daily bars were hairlines). */
const WEEKLY_AFTER_DAYS = 120;
const WEEKLY_AFTER_DAYS_PHONE = 45;

const DEFAULT_RANGE: ReadingStatsRange = "90d";

/**
 * The reading log's Stats tab (`/log/stats`, folio-flair phase 4): the
 * deep-dive that used to live in Settings → Reading stats. A sticky range
 * selector over a band of headline figures, then cards for when you read,
 * activity over time, rankings, pace and rereads. Everything except the
 * streaks is scoped to the selected range.
 */
export function StatsTab({ initialRange }: { initialRange?: string }) {
  const [range, setRangeState] = useState<ReadingStatsRange>(() =>
    parseRange(initialRange, DEFAULT_RANGE),
  );
  // The range lives in `?range=` so a reload or a shared link keeps it
  // (replaceState: no history entry, no server round-trip).
  const setRange = (next: ReadingStatsRange) => {
    setRangeState(next);
    const url = new URL(window.location.href);
    if (next === DEFAULT_RANGE) url.searchParams.delete("range");
    else url.searchParams.set("range", next);
    window.history.replaceState(window.history.state, "", url.toString());
  };
  const stats = useReadingStats({ type: "all" }, range);
  const reduced = useReducedMotion();
  const data = stats.data;

  return (
    <div className="space-y-6">
      <RangeBar range={range} onChange={setRange} data={data} />

      {stats.isLoading ? (
        <StatsBodySkeleton />
      ) : !data ? (
        <p className="text-destructive text-sm">
          Couldn&apos;t load your reading stats. Try again in a moment.
        </p>
      ) : (
        <div className="space-y-6">
          <HeroCards data={data} />
          {data.totals.sessions === 0 ? (
            <EmptyRange all={range === "all"} />
          ) : (
            <StatsBody data={data} range={range} animate={!reduced} />
          )}
        </div>
      )}
    </div>
  );
}

function StatsBody({
  data,
  range,
  animate,
}: {
  data: ReadingStatsView;
  range: ReadingStatsRange;
  animate: boolean;
}) {
  const phone = useIsPhone();
  const series = activitySeries(
    data,
    range,
    phone ? WEEKLY_AFTER_DAYS_PHONE : WEEKLY_AFTER_DAYS,
  );
  return (
    <>
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-5">
        <StatCard
          title="Weekly rhythm"
          description="When your sessions land, by weekday and hour."
          className="lg:col-span-3"
        >
          <DowHourHeatmap cells={data.dow_hour ?? []} />
        </StatCard>
        <StatCard
          title="Time of day"
          description="How your reading time splits across the day."
          className="lg:col-span-2"
        >
          {data.time_of_day ? (
            <TimeOfDayDonut data={data.time_of_day} animate={animate} />
          ) : (
            <p className="text-muted-foreground text-sm">
              Read a few sessions to see when you usually pick up a book.
            </p>
          )}
        </StatCard>
      </div>

      <StatCard
        title={series.weekly ? "Active time per week" : "Active time per day"}
        description={
          series.weekly
            ? "Minutes read each week across the range."
            : "Minutes read each day across the range."
        }
      >
        <PerDayBarChart
          data={series.buckets}
          height={200}
          animate={animate}
          bucket={series.weekly ? "week" : "day"}
        />
      </StatCard>

      <StatCard
        title="Top rankings"
        description="What you spent the most time with — switch the dimension, or rank by sessions."
      >
        <TopRankingsList data={data} />
      </StatCard>

      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        <StatCard
          title="Reading pace"
          description="Seconds per page, session by session, with a 7-session average."
        >
          <PaceChart points={data.pace_series ?? []} height={220} />
        </StatCard>
        <StatCard
          title="Most reread"
          description="The issues and series you keep coming back to."
        >
          <RereadList
            issues={data.reread_top_issues ?? []}
            series={data.reread_top_series ?? []}
          />
        </StatCard>
      </div>
    </>
  );
}

/** Card shell for one Stats section: title + one-line description. */
function StatCard({
  title,
  description,
  className,
  children,
}: {
  title: string;
  description: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <section
      className={cn(
        "border-border bg-card flex min-w-0 flex-col rounded-xl border p-4 sm:p-5",
        className,
      )}
    >
      <header className="mb-4">
        <h2 className="text-foreground text-lg font-semibold tracking-tight">
          {title}
        </h2>
        <p className="text-muted-foreground mt-0.5 text-sm">{description}</p>
      </header>
      {children}
    </section>
  );
}

/**
 * The range selector row. Pins under the topbar while you scroll (when it
 * fits on one row) and only then takes the chrome band's tint and border.
 */
function RangeBar({
  range,
  onChange,
  data,
}: {
  range: ReadingStatsRange;
  onChange: (next: ReadingStatsRange) => void;
  data: ReadingStatsView | undefined;
}) {
  const { ref, sentinelRef, sticky, stuck } = useStickyToolbar({});
  const span = data ? rangeSpan(data, range) : null;
  return (
    <>
      <div ref={sentinelRef} aria-hidden="true" />
      <div
        ref={ref}
        data-toolbar-row=""
        data-stuck={stuck ? "" : undefined}
        className={cn(
          "-mx-4 -mt-2 flex items-center justify-between gap-3 border-b border-transparent px-4 py-2 transition-colors md:-mx-8 md:px-8",
          sticky && "sticky top-(--topbar-h) z-20",
          stuck && CHROME_BAND_CLASS,
        )}
      >
        <p className="text-muted-foreground min-w-0 truncate text-sm max-sm:hidden">
          <span className="text-foreground font-medium">
            {RANGE_CAPTION[range]}
          </span>
          {span ? <span> · {span}</span> : null}
        </p>
        <div className="min-w-0 max-sm:flex-1">
          <ActivityRangeSelector value={range} onChange={onChange} />
        </div>
      </div>
    </>
  );
}

function EmptyRange({ all }: { all: boolean }) {
  return (
    <div className="border-border bg-card flex flex-col items-center gap-2 rounded-xl border border-dashed px-6 py-12 text-center">
      <BarChart3 aria-hidden className="text-muted-foreground size-8" />
      <p className="text-foreground font-medium">
        {all ? "No reading recorded yet" : "No reading in this range"}
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        {all
          ? "Open an issue and your sessions, pace and rankings start filling in here."
          : "Pick a longer range above to see your stats."}
      </p>
    </div>
  );
}

function StatsBodySkeleton() {
  return (
    <div className="space-y-6" aria-hidden>
      <Skeleton className="h-28 w-full rounded-xl" />
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-5">
        <Skeleton className="h-64 rounded-xl lg:col-span-3" />
        <Skeleton className="h-64 rounded-xl lg:col-span-2" />
      </div>
      <Skeleton className="h-72 w-full rounded-xl" />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Range helpers

/** Today's date (`YYYY-MM-DD`) in the stats' timezone. */
function todayIn(timezone: string): string {
  try {
    return new Intl.DateTimeFormat("en-CA", {
      timeZone: timezone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    }).format(new Date());
  } catch {
    return isoDate(new Date());
  }
}

function isoDate(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function parseIso(iso: string): Date {
  return new Date(`${iso}T00:00:00`);
}

function addDays(iso: string, n: number): string {
  const d = parseIso(iso);
  d.setDate(d.getDate() + n);
  return isoDate(d);
}

/** First and last day the range covers, or null when there's no anchor. */
function rangeBounds(
  data: ReadingStatsView,
  range: string,
): { start: string; end: string } | null {
  const today = todayIn(data.timezone);
  // Calendar year (`range=YYYY`): Jan 1 to Dec 31, capped at today.
  if (/^\d{4}$/.test(range)) {
    const end = `${range}-12-31`;
    return { start: `${range}-01-01`, end: end < today ? end : today };
  }
  const days = RANGE_DAYS[range as ReadingStatsRange];
  if (days) return { start: addDays(today, -(days - 1)), end: today };
  const first = data.per_day[0]?.date ?? data.first_read_at?.slice(0, 10);
  return first ? { start: first, end: today } : null;
}

function rangeSpan(data: ReadingStatsView, range: string): string | null {
  const b = rangeBounds(data, range);
  if (!b) return null;
  const s = parseIso(b.start);
  const e = parseIso(b.end);
  const sameYear = s.getFullYear() === e.getFullYear();
  const fmt = (d: Date, withYear: boolean) =>
    d.toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
      ...(withYear ? { year: "numeric" } : {}),
    });
  return `${fmt(s, !sameYear)} – ${fmt(e, true)}`;
}

/**
 * The server's `per_day` lists only days with reading; fill the gaps so the
 * chart shows time passing, and fold long ranges into Monday-start weeks.
 */
function activitySeries(
  data: ReadingStatsView,
  range: string,
  weeklyAfter: number,
): { buckets: ReadingDayBucket[]; weekly: boolean } {
  const bounds = rangeBounds(data, range);
  if (!bounds) return { buckets: [...data.per_day], weekly: false };
  const byDate = new Map(data.per_day.map((d) => [d.date, d]));
  const days: ReadingDayBucket[] = [];
  for (let d = bounds.start; d <= bounds.end; d = addDays(d, 1)) {
    days.push(
      byDate.get(d) ?? { date: d, active_ms: 0, sessions: 0, pages: 0 },
    );
  }
  if (days.length <= weeklyAfter) return { buckets: days, weekly: false };

  const weeks = new Map<string, ReadingDayBucket>();
  for (const day of days) {
    const dt = parseIso(day.date);
    const monday = addDays(day.date, -((dt.getDay() + 6) % 7));
    const w = weeks.get(monday) ?? {
      date: monday,
      active_ms: 0,
      sessions: 0,
      pages: 0,
    };
    w.active_ms += day.active_ms;
    w.sessions += day.sessions;
    w.pages = (w.pages ?? 0) + (day.pages ?? 0);
    weeks.set(monday, w);
  }
  return { buckets: [...weeks.values()], weekly: true };
}
