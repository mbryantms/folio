"use client";

import { ArrowRight, Flame } from "lucide-react";
import Link from "next/link";
import { useMemo } from "react";

import { CountUp } from "@/components/CountUp";
import { Cover } from "@/components/Cover";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingLogInfinite, useReadingStats } from "@/lib/api/queries";
import type { ReadingLogEventView, ReadingStatsRange } from "@/lib/api/types";
import {
  dayLabel,
  localDate,
  milestones,
  readingYears,
  streakRuns,
  type DayPages,
  type Milestone,
} from "@/lib/log/overview";
import { issueUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

import { OverviewSkeleton } from "../LogSkeletons";
import { Milestones } from "./Milestones";
import { ReadingHeatmap } from "./ReadingHeatmap";

/** A finished issue from the reading log, flattened for this page. */
type Finished = {
  date: string;
  cover: string | null;
  seriesId: string | null;
  seriesName: string;
  number: string | null;
  pages: number;
  href: string | null;
};

/**
 * The reading log's Overview tab (folio-flair phase 4): a fixed, designed
 * page — this year's numbers, the year in review, a year of activity,
 * milestones and recent reading. Plan:
 * ~/.claude/plans/folio-flair-phase4-log.md
 */
export function OverviewPage() {
  const all = useReadingStats({ type: "all" }, "all");
  const timeZone = all.data?.timezone ?? "UTC";
  const today = localDate(new Date(), timeZone);
  const thisYear = +today.slice(0, 4);
  const yearStats = useReadingStats(
    { type: "all" },
    String(thisYear) as ReadingStatsRange,
  );
  const finishedQ = useReadingLogInfinite({
    kinds: ["issue_finished"],
    limit: 60,
  });
  const seriesQ = useReadingLogInfinite({
    kinds: ["series_finished"],
    limit: 12,
  });

  const days: DayPages[] = useMemo(
    () =>
      (all.data?.per_day ?? []).map((d) => ({ date: d.date, pages: d.pages })),
    [all.data],
  );
  const runs = useMemo(() => streakRuns(days, today), [days, today]);

  const finished: Finished[] = useMemo(
    () =>
      (finishedQ.data?.pages ?? [])
        .flatMap((p) => p.events)
        .map((e) => toFinished(e, timeZone)),
    [finishedQ.data, timeZone],
  );
  const seriesFinished = useMemo(
    () =>
      (seriesQ.data?.pages ?? [])
        .flatMap((p) => p.events)
        .flatMap((e) =>
          e.payload.kind === "series_finished" && e.series
            ? [
                {
                  seriesId: e.series.id,
                  name: e.series.name,
                  totalIssues: e.payload.total_issues,
                  at: localDate(new Date(e.occurred_at), timeZone),
                  cover: e.series.cover_url ?? null,
                },
              ]
            : [],
        ),
    [seriesQ.data, timeZone],
  );

  // The year the page is about: this year, or the latest year with reading
  // when this one has none yet (early January).
  const years = readingYears(days);
  const yearPages = days
    .filter((d) => d.date.startsWith(`${thisYear}-`))
    .reduce((s, d) => s + d.pages, 0);
  const yearDays = days.filter(
    (d) => d.date.startsWith(`${thisYear}-`) && d.pages > 0,
  ).length;
  const reviewYear = yearPages > 0 ? thisYear : (years[0] ?? thisYear);

  const ms = useMemo(
    () => milestones({ days, today, year: thisYear, seriesFinished }),
    [days, today, thisYear, seriesFinished],
  );
  const recentCovers = finished.flatMap((f) => (f.cover ? [f.cover] : []));
  const coversFor = (m: Milestone): string[] => {
    let pool: string[] = [];
    if (m.seriesId) {
      const s = seriesFinished.find((x) => x.seriesId === m.seriesId);
      pool = finished
        .filter((f) => f.seriesId === m.seriesId && f.cover)
        .map((f) => f.cover!);
      if (s?.cover) pool.unshift(s.cover);
    } else if (m.window) {
      const { from, to } = m.window;
      pool = finished
        .filter((f) => f.date >= from && f.date <= to && f.cover)
        .map((f) => f.cover!);
    }
    return dedupe(pool.length >= 3 ? pool : [...pool, ...recentCovers]).slice(
      0,
      8,
    );
  };

  const issuesByDate = useMemo(() => {
    const m = new Map<string, number>();
    for (const f of finished) m.set(f.date, (m.get(f.date) ?? 0) + 1);
    return m;
  }, [finished]);

  if (all.isLoading) return <OverviewSkeleton />;
  if (all.isError || !all.data) {
    return (
      <p className="text-destructive text-sm">
        Couldn&apos;t load your reading stats.
      </p>
    );
  }
  const totals = all.data.totals;
  const longest = runs.longest;

  return (
    <div className="space-y-10">
      <section aria-label={`${thisYear} at a glance`} className="space-y-4">
        <p className="text-muted-foreground text-sm">
          {yearDays > 0
            ? `${yearDays} reading ${yearDays === 1 ? "day" : "days"} in ${thisYear}`
            : `No reading yet in ${thisYear}`}
        </p>
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          <StatTile
            label={`Pages in ${thisYear}`}
            value={yearPages}
            sub={`${yearDays} reading days`}
          />
          <StatTile
            label="Issues read"
            value={yearStats.data?.totals.distinct_issues ?? null}
            sub={
              yearStats.data
                ? `${Math.round(yearStats.data.totals.active_ms / 3_600_000)} hours reading`
                : `in ${thisYear}`
            }
          />
          <StatTile
            label="Current streak"
            value={totals.current_streak}
            unit={totals.current_streak === 1 ? "day" : "days"}
            sub={
              runs.current
                ? `since ${dayLabel(runs.current.start, String(thisYear))}`
                : "read today to start one"
            }
            accent={totals.current_streak > 0}
          />
          <StatTile
            label="Longest streak"
            value={totals.longest_streak}
            unit={totals.longest_streak === 1 ? "day" : "days"}
            sub={
              longest
                ? `${dayLabel(longest.start, String(thisYear))} – ${dayLabel(longest.end, String(thisYear))}`
                : "—"
            }
          />
        </div>
      </section>

      <YearBanner
        year={reviewYear}
        pages={
          reviewYear === thisYear
            ? yearPages
            : days
                .filter((d) => d.date.startsWith(`${reviewYear}-`))
                .reduce((s, d) => s + d.pages, 0)
        }
        longest={longest?.length ?? 0}
        covers={topCovers(finished, reviewYear)}
      />

      <Section
        title="Activity"
        description="A year of reading, one square a day."
      >
        <div className="bg-card rounded-xl border p-5">
          <ReadingHeatmap
            days={days}
            today={today}
            current={runs.current}
            issuesByDate={issuesByDate}
          />
        </div>
      </Section>

      <Section
        title="Milestones"
        description="Streaks, big months and finished series. Replay any you've earned."
      >
        <Milestones items={ms} coversFor={coversFor} />
      </Section>

      <Section
        title="Recent reading"
        description="What you finished, day by day."
        action={
          <Link
            href="/log/activity"
            className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-sm transition-colors"
          >
            Everything you&apos;ve finished
            <ArrowRight className="size-3.5" aria-hidden />
          </Link>
        }
      >
        <RecentReading
          finished={finished}
          loading={finishedQ.isLoading}
          current={runs.current}
          thisYear={String(thisYear)}
        />
      </Section>
    </div>
  );
}

function Section({
  title,
  description,
  action,
  children,
}: {
  title: string;
  description?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-4">
      <div className="flex flex-wrap items-end justify-between gap-2">
        <div>
          <h2 className="text-lg font-semibold tracking-tight">{title}</h2>
          {description ? (
            <p className="text-muted-foreground text-sm">{description}</p>
          ) : null}
        </div>
        {action}
      </div>
      {children}
    </section>
  );
}

function StatTile({
  label,
  value,
  unit,
  sub,
  accent = false,
}: {
  label: string;
  value: number | null;
  unit?: string;
  sub: string;
  accent?: boolean;
}) {
  return (
    <div className="bg-card min-w-0 rounded-xl border p-4 sm:p-5">
      <p className="text-muted-foreground flex items-center gap-1.5 text-xs font-medium tracking-wide uppercase">
        {accent ? (
          <Flame className="text-primary size-3.5" aria-hidden />
        ) : null}
        {label}
      </p>
      <p className="mt-1.5 text-3xl font-semibold tracking-tight tabular-nums">
        {value == null ? "—" : <CountUp value={value} />}
        {unit ? (
          <span className="text-muted-foreground ml-1 text-base font-normal">
            {unit}
          </span>
        ) : null}
      </p>
      <p className="text-muted-foreground mt-0.5 truncate text-xs">{sub}</p>
    </div>
  );
}

function YearBanner({
  year,
  pages,
  longest,
  covers,
}: {
  year: number;
  pages: number;
  longest: number;
  covers: string[];
}) {
  return (
    <Link
      href={`/log/year/${year}`}
      className={cn(
        "group relative flex items-center gap-4 overflow-hidden rounded-2xl p-5 sm:p-6",
        "text-primary-foreground shadow-[0_18px_40px_-20px_var(--primary)]",
        "bg-[linear-gradient(120deg,var(--primary),color-mix(in_oklab,var(--primary)_70%,#7c2d12))]",
        "focus-visible:ring-ring transition-transform focus-visible:ring-2 focus-visible:outline-none motion-safe:hover:-translate-y-0.5",
      )}
    >
      <div className="relative z-10 min-w-0 flex-1">
        <p className="text-xs font-bold tracking-[0.08em] uppercase opacity-80">
          {year} · Year in review
        </p>
        <h2 className="mt-1 text-xl font-semibold tracking-tight sm:text-2xl">
          Your {year} in comics
        </h2>
        <p className="mt-1 text-sm opacity-90">
          {pages.toLocaleString()} pages
          {longest ? ` and a ${longest}-day best streak` : ""}. Tap to play.
        </p>
      </div>
      {covers.length ? (
        <div
          aria-hidden
          className="relative hidden h-24 w-36 shrink-0 sm:block"
        >
          {covers.slice(0, 3).map((src, k) => (
            // eslint-disable-next-line @next/next/no-img-element
            <img
              key={src}
              src={src}
              alt=""
              className="absolute bottom-[-1.5rem] left-1/2 aspect-[2/3] w-16 rounded-md object-cover shadow-lg transition-transform duration-300 group-hover:-translate-y-1"
              style={{
                translate: `calc(-50% + ${(k - 1) * 34}px) 0`,
                rotate: `${(k - 1) * 9}deg`,
                zIndex: k === 1 ? 2 : 1,
              }}
            />
          ))}
        </div>
      ) : null}
      <ArrowRight
        aria-hidden
        className="relative z-10 size-5 shrink-0 transition-transform group-hover:translate-x-0.5"
      />
    </Link>
  );
}

/** Finished issues grouped by day, newest first. */
function RecentReading({
  finished,
  loading,
  current,
  thisYear,
}: {
  finished: Finished[];
  loading: boolean;
  current: { start: string; end: string } | null;
  thisYear: string;
}) {
  const byDay = useMemo(() => {
    const groups: { date: string; items: Finished[] }[] = [];
    for (const f of finished) {
      const g = groups[groups.length - 1];
      if (g && g.date === f.date) g.items.push(f);
      else groups.push({ date: f.date, items: [f] });
    }
    return groups.slice(0, 10);
  }, [finished]);

  if (loading) return <Skeleton className="h-64 w-full rounded-xl" />;
  if (!byDay.length) {
    return (
      <p className="bg-card text-muted-foreground rounded-xl border p-6 text-sm">
        Finished issues will show up here.
      </p>
    );
  }
  return (
    <ul className="bg-card divide-border divide-y rounded-xl border">
      {byDay.map((g) => {
        const first = g.items[0]!;
        const pages = g.items.reduce((s, f) => s + f.pages, 0);
        const inStreak =
          !!current && g.date >= current.start && g.date <= current.end;
        const body = (
          <>
            <div className="w-10 shrink-0">
              <Cover src={first.cover} alt="" fallback={first.seriesName} />
            </div>
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium">
                {first.seriesName}
                {first.number ? ` #${first.number}` : ""}
                {g.items.length > 1 ? (
                  <span className="text-muted-foreground font-normal">
                    {" "}
                    +{g.items.length - 1} more
                  </span>
                ) : null}
              </p>
              <p className="text-muted-foreground flex items-center gap-2 text-xs">
                {dayLabel(g.date, thisYear)}
                {inStreak ? (
                  <span className="text-primary inline-flex items-center gap-0.5 font-medium">
                    <Flame className="size-3" aria-hidden />
                    streak
                  </span>
                ) : null}
              </p>
            </div>
            <p className="shrink-0 text-right">
              <span className="block text-sm font-semibold tabular-nums">
                {pages.toLocaleString()}
              </span>
              <span className="text-muted-foreground text-xs">pages</span>
            </p>
          </>
        );
        return (
          <li key={g.date}>
            {first.href ? (
              <Link
                href={first.href}
                className="hover:bg-accent/40 flex items-center gap-3 px-4 py-3 transition-colors"
              >
                {body}
              </Link>
            ) : (
              <div className="flex items-center gap-3 px-4 py-3">{body}</div>
            )}
          </li>
        );
      })}
    </ul>
  );
}

function toFinished(e: ReadingLogEventView, timeZone: string): Finished {
  return {
    date: localDate(new Date(e.occurred_at), timeZone),
    cover: e.issue?.cover_url ?? e.series?.cover_url ?? null,
    seriesId: e.series?.id ?? null,
    seriesName: e.series?.name ?? e.issue?.title ?? "Untitled",
    number: e.issue?.number ?? null,
    pages: e.issue?.page_count ?? 0,
    href:
      e.series && e.issue
        ? issueUrl({ series_slug: e.series.slug, slug: e.issue.slug })
        : null,
  };
}

/** Covers of the year's most-finished series (latest issue's cover). */
function topCovers(finished: Finished[], year: number): string[] {
  const tally = new Map<string, { n: number; cover: string }>();
  for (const f of finished) {
    if (!f.date.startsWith(`${year}-`) || !f.cover || !f.seriesId) continue;
    const t = tally.get(f.seriesId);
    if (t) t.n += 1;
    else tally.set(f.seriesId, { n: 1, cover: f.cover });
  }
  return [...tally.values()].sort((a, b) => b.n - a.n).map((t) => t.cover);
}

function dedupe(xs: string[]): string[] {
  return [...new Set(xs)];
}
