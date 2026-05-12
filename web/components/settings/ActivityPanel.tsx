"use client";

import dynamic from "next/dynamic";
import { useState } from "react";

import { ActivityHeatmap } from "@/components/activity/ActivityHeatmap";
import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
import { ActivityTimeline } from "@/components/activity/ActivityTimeline";
import { DowHourHeatmap } from "@/components/activity/DowHourHeatmap";
import { HeroCards } from "@/components/activity/HeroCards";
import { RereadList } from "@/components/activity/RereadList";
import { TimeOfDayDonut } from "@/components/activity/TimeOfDayDonut";
import { TopRankingsList } from "@/components/activity/TopRankings";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import type { ReadingStatsRange } from "@/lib/api/types";

import { PrivacyControls } from "./PrivacyControls";
import { SettingsSection } from "./SettingsSection";

// Recharts is heavy and SSR-hostile; load only when these views are on screen.
const PerDayBarChart = dynamic(
  () =>
    import("@/components/activity/PerDayBarChart").then(
      (m) => m.PerDayBarChart,
    ),
  { ssr: false, loading: () => <Skeleton className="h-32 w-full" /> },
);
const PaceChart = dynamic(
  () => import("@/components/activity/PaceChart").then((m) => m.PaceChart),
  { ssr: false, loading: () => <Skeleton className="h-48 w-full" /> },
);

export function ActivityPanel() {
  const [range, setRange] = useState<ReadingStatsRange>("90d");
  // Heatmap pins to 1y so the year-back grid is always present even when
  // the range selector is short.
  const yearStats = useReadingStats({ type: "all" }, "1y");
  const stats = useReadingStats({ type: "all" }, range);

  return (
    <div className="space-y-8">
      <SettingsSection
        title="Overview"
        description="Headline numbers across your selected window plus your global streaks."
      >
        <div className="space-y-4">
          <ActivityRangeSelector value={range} onChange={setRange} />
          {stats.isLoading ? (
            <Skeleton className="h-32 w-full" />
          ) : stats.data ? (
            <HeroCards data={stats.data} />
          ) : (
            <p className="text-destructive text-sm">Failed to load stats.</p>
          )}
        </div>
      </SettingsSection>

      <SettingsSection
        title="Year heatmap"
        description="Every cell is a day in your timezone. Brighter = more time. Anchored to the last 53 weeks regardless of the range above."
      >
        {yearStats.isLoading ? (
          <Skeleton className="h-32 w-full" />
        ) : yearStats.data ? (
          <ActivityHeatmap perDay={yearStats.data.per_day} />
        ) : (
          <p className="text-destructive text-sm">Failed to load heatmap.</p>
        )}
      </SettingsSection>

      <SettingsSection
        title="When you read"
        description="Where your sessions cluster across the week, and how that breaks down into rough times of day."
      >
        <div className="grid grid-cols-1 gap-6 xl:grid-cols-2">
          {stats.isLoading ? (
            <Skeleton className="h-44 w-full" />
          ) : stats.data ? (
            <DowHourHeatmap cells={stats.data.dow_hour} />
          ) : null}
          {stats.isLoading ? (
            <Skeleton className="h-44 w-full" />
          ) : stats.data ? (
            <TimeOfDayDonut data={stats.data.time_of_day} />
          ) : null}
        </div>
      </SettingsSection>

      <SettingsSection
        title="Active time per day"
        description="One bar per day in the selected window."
      >
        {stats.isLoading ? (
          <Skeleton className="h-32 w-full" />
        ) : stats.data && stats.data.per_day.length > 0 ? (
          <PerDayBarChart data={stats.data.per_day} />
        ) : (
          <p className="text-muted-foreground text-sm">
            No reading recorded in this window yet.
          </p>
        )}
      </SettingsSection>

      <SettingsSection
        title="Reading pace"
        description="Seconds per page per session, with a 7-session moving average laid over the raw points."
      >
        {stats.isLoading ? (
          <Skeleton className="h-48 w-full" />
        ) : stats.data ? (
          <PaceChart points={stats.data.pace_series} />
        ) : null}
      </SettingsSection>

      <SettingsSection
        title="Top rankings"
        description="Series, creators, publishers, imprints, genres, and tags by accumulated reading time. Switch dimensions or sort by sessions instead of time."
      >
        {stats.isLoading ? (
          <Skeleton className="h-48 w-full" />
        ) : stats.data ? (
          <TopRankingsList data={stats.data} />
        ) : null}
      </SettingsSection>

      <SettingsSection
        title="Most reread"
        description="Issues and series you've returned to more than once."
      >
        {stats.isLoading ? (
          <Skeleton className="h-32 w-full" />
        ) : stats.data ? (
          <RereadList
            issues={stats.data.reread_top_issues}
            series={stats.data.reread_top_series}
          />
        ) : null}
      </SettingsSection>

      <SettingsSection
        title="Recent sessions"
        description="Each row is a single reading session — click an issue to keep going."
      >
        <ActivityTimeline scope={{ type: "all" }} />
      </SettingsSection>

      <SettingsSection
        title="Privacy"
        description="Control whether new sessions are captured, whether your activity counts in server-wide aggregates, and reset your history."
      >
        <PrivacyControls />
      </SettingsSection>
    </div>
  );
}
