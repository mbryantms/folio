"use client";

import { useRouter } from "next/navigation";
import { useEffect, useMemo } from "react";

import { useReadingLogInfinite, useReadingStats } from "@/lib/api/queries";
import type { ReadingStatsRange } from "@/lib/api/types";
import { dayDiff, localDate, type DayPages } from "@/lib/log/overview";
import {
  biggestDay,
  busiestMonth,
  daysInYear,
  inYear,
  monthBars,
  mosaicCovers,
  pickerYears,
  readingRhythm,
  topSeriesWithCovers,
  yearFetchWindow,
  yearLongestRun,
  type FinishedCover,
} from "@/lib/log/year";

import { YearStory, type YearModel } from "./YearStory";

/** Reading-log pages fetched for covers (100 events each, the server cap). */
const MAX_LOG_PAGES = 4;

/**
 * Data for the Year in review story: the year-scoped stats (`range=YYYY`),
 * the all-time buckets (for the year picker) and the year's finished
 * issues (for covers). Plan: ~/.claude/plans/folio-flair-phase4-log.md
 */
export function YearReview({ year }: { year: number }) {
  const router = useRouter();
  const stats = useReadingStats(
    { type: "all" },
    String(year) as ReadingStatsRange,
  );
  const all = useReadingStats({ type: "all" }, "all");
  const win = useMemo(() => yearFetchWindow(year), [year]);
  const log = useReadingLogInfinite({
    kinds: ["issue_finished"],
    from: win.from,
    to: win.to,
    limit: 100,
  });

  // Pull a few pages so the mosaic and top-series covers see most of the
  // year; past that the mosaic says "and more".
  const pagesLoaded = log.data?.pages.length ?? 0;
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = log;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && pagesLoaded < MAX_LOG_PAGES)
      void fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, pagesLoaded, fetchNextPage]);

  const timeZone = stats.data?.timezone ?? all.data?.timezone ?? "UTC";

  const finished: FinishedCover[] = useMemo(
    () =>
      (log.data?.pages ?? [])
        .flatMap((p) => p.events)
        .filter((e) => inYear(e.occurred_at, year, timeZone))
        .map((e) => ({
          id: e.id,
          issueId: e.issue?.id ?? null,
          seriesId: e.series?.id ?? null,
          seriesName: e.series?.name ?? e.issue?.title ?? "Untitled",
          cover: e.issue?.cover_url ?? null,
          seriesCover: e.series?.cover_url ?? null,
        })),
    [log.data, year, timeZone],
  );

  const years = useMemo(
    () =>
      pickerYears(
        (all.data?.per_day ?? []).map((d) => ({
          date: d.date,
          pages: d.pages,
        })),
        year,
      ),
    [all.data, year],
  );

  const model: YearModel | null = useMemo(() => {
    const s = stats.data;
    if (!s) return null;
    const days: DayPages[] = s.per_day.map((d) => ({
      date: d.date,
      pages: d.pages,
    }));
    const today = localDate(new Date(), timeZone);
    const thisYear = +today.slice(0, 4);
    const months = monthBars(days, year);
    const mosaic = mosaicCovers(finished, 36);
    const pages = days.reduce((sum, d) => sum + d.pages, 0);
    return {
      year,
      inProgress: year === thisYear,
      pages,
      issues: s.totals.distinct_issues,
      activeMs: s.totals.active_ms,
      sessions: s.totals.sessions,
      daysActive: s.totals.days_active || days.length,
      // The year so far while it is still running.
      daysInScope:
        year === thisYear
          ? dayDiff(`${year}-01-01`, today) + 1
          : daysInYear(year),
      months,
      busiest: busiestMonth(months),
      bigDay: biggestDay(days),
      run: yearLongestRun(days, year),
      rhythm: readingRhythm(s.dow_hour ?? []),
      top: topSeriesWithCovers(s.top_series ?? [], finished),
      mosaic: { ...mosaic, more: !!log.hasNextPage },
      coversLoading: log.isLoading,
    };
  }, [stats.data, timeZone, year, finished, log.hasNextPage, log.isLoading]);

  return (
    <YearStory
      // A different year starts again from the first slide.
      key={year}
      year={year}
      model={model}
      loading={stats.isLoading}
      error={stats.isError}
      years={years}
      onPickYear={(y) => router.push(`/log/year/${y}`)}
      onClose={() => router.push("/log")}
    />
  );
}
