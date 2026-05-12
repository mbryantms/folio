"use client";

import dynamic from "next/dynamic";

import { ActivityTimeline } from "@/components/activity/ActivityTimeline";
import { IssueGridHeatmap } from "@/components/activity/IssueGridHeatmap";
import { TopRankingsList } from "@/components/activity/TopRankings";
import { Skeleton } from "@/components/ui/skeleton";
import { formatTotalHours } from "@/lib/activity";
import { useReadingStats } from "@/lib/api/queries";
import type { IssueSummaryView } from "@/lib/api/types";

const PaceChart = dynamic(
  () => import("@/components/activity/PaceChart").then((m) => m.PaceChart),
  { ssr: false, loading: () => <Skeleton className="h-48 w-full" /> },
);

/**
 * Series-scoped activity tab. Aggregates stats for this series, shows an
 * issue grid heatmap (one cell per issue, colored by read count), pace,
 * top creators, and the per-session timeline.
 */
export function SeriesActivityTab({
  seriesId,
  seriesSlug,
  issues,
  totalIssueCount,
}: {
  seriesId: string;
  seriesSlug: string;
  issues: ReadonlyArray<IssueSummaryView>;
  /** Total issues in the series (for the "issues read / total" stat). The
   *  `issues` prop is the first page only; the count comes from the parent's
   *  `series.issue_count` so the denominator matches the series header. */
  totalIssueCount: number | null;
}) {
  const stats = useReadingStats({ type: "series", id: seriesId }, "all");

  if (stats.isLoading) return <Skeleton className="h-64 w-full" />;
  if (!stats.data) {
    return (
      <p className="text-destructive text-sm">Failed to load series stats.</p>
    );
  }

  const data = stats.data;
  const hasActivity = data.totals.sessions > 0;

  return (
    <div className="space-y-6">
      <StatCards
        data={data}
        totalIssueCount={totalIssueCount ?? issues.length}
      />

      <section>
        <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Issue grid
        </h3>
        <IssueGridHeatmap
          issues={issues}
          rereads={data.reread_top_issues}
          seriesSlug={seriesSlug}
        />
      </section>

      <section>
        <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Reading pace
        </h3>
        {hasActivity ? (
          <PaceChart points={data.pace_series} />
        ) : (
          <p className="text-muted-foreground text-sm">
            Start an issue to record your first pace sample.
          </p>
        )}
      </section>

      <section>
        <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Top creators &amp; tags in this series
        </h3>
        <TopRankingsList
          data={data}
          dimensions={[
            "writer",
            "penciller",
            "inker",
            "colorist",
            "letterer",
            "cover_artist",
            "genres",
            "tags",
          ]}
          emptyHint="No tag/credit metadata recorded for this series."
        />
      </section>

      <section>
        <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Recent sessions
        </h3>
        <ActivityTimeline
          scope={{ type: "series", id: seriesId }}
          emptyHint="No reading sessions for this series yet — start an issue to record your first."
        />
      </section>
    </div>
  );
}

function StatCards({
  data,
  totalIssueCount,
}: {
  data: import("@/lib/api/types").ReadingStatsView;
  totalIssueCount: number;
}) {
  const completionPct =
    totalIssueCount > 0
      ? Math.round((data.completion.completed / totalIssueCount) * 100)
      : 0;
  const cards: ReadonlyArray<{ label: string; value: string; sub: string }> = [
    {
      label: "Issues read",
      value: `${data.totals.distinct_issues} / ${totalIssueCount}`,
      sub: `${completionPct}% complete (${data.completion.completed} finished)`,
    },
    {
      label: "Total time",
      value: formatTotalHours(data.totals.active_ms / 3_600_000),
      sub: `${data.totals.sessions} session${data.totals.sessions === 1 ? "" : "s"}`,
    },
    {
      label: "Pages read",
      value: data.totals.distinct_pages_read.toLocaleString(),
      sub: data.last_read_at
        ? `last read ${new Date(data.last_read_at).toLocaleDateString()}`
        : "no sessions yet",
    },
    {
      label: "Streak",
      value: `${data.totals.current_streak}d`,
      sub: `longest ${data.totals.longest_streak}d`,
    },
  ];
  return (
    <ul className="grid grid-cols-2 gap-3 md:grid-cols-4">
      {cards.map((c) => (
        <li
          key={c.label}
          className="border-border bg-card rounded-md border p-3"
        >
          <p className="text-muted-foreground text-xs tracking-wide uppercase">
            {c.label}
          </p>
          <p className="text-foreground mt-1 text-lg font-semibold">
            {c.value}
          </p>
          <p className="text-muted-foreground mt-0.5 text-xs">{c.sub}</p>
        </li>
      ))}
    </ul>
  );
}
