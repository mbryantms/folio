"use client";

import dynamic from "next/dynamic";
import Link from "next/link";

import { StatCard } from "@/components/admin/StatCard";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDurationMs } from "@/lib/activity";
import { useAdminOverview } from "@/lib/api/queries";
import type { AdminOverviewView } from "@/lib/api/types";

const PerDayBarChart = dynamic(
  () =>
    import("@/components/activity/PerDayBarChart").then(
      (m) => m.PerDayBarChart,
    ),
  { ssr: false, loading: () => <Skeleton className="h-32 w-full" /> },
);

export function StatsClient() {
  const overview = useAdminOverview({ intervalMs: 60_000 });

  if (overview.isLoading || !overview.data) {
    return <Skeleton className="h-72 w-full" />;
  }
  if (overview.error) {
    return <p className="text-destructive text-sm">Failed to load stats.</p>;
  }
  const data = overview.data;

  return (
    <div className="space-y-6">
      <TotalsRow data={data} />
      <ReadsChartCard data={data} />
      <TopSeriesCard data={data} />
    </div>
  );
}

function TotalsRow({ data }: { data: AdminOverviewView }) {
  return (
    <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
      <StatCard
        label="Sessions today"
        value={data.sessions_today}
        hint="reading sessions in the last 24h"
      />
      <StatCard
        label="Reading now"
        value={data.active_readers_now}
        hint="distinct users, 5m heartbeat"
      />
      <StatCard
        label="Issues"
        value={data.totals.issues.toLocaleString()}
        hint="active, on disk"
      />
      <StatCard label="Users" value={data.totals.users} hint="local + OIDC" />
    </div>
  );
}

function ReadsChartCard({ data }: { data: AdminOverviewView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Reads — last 14 days
        </CardTitle>
        <p className="text-muted-foreground text-xs">
          Daily session count and active reading time across all users.
        </p>
      </CardHeader>
      <CardContent>
        {data.reads_per_day.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No reading recorded in the last 14 days.
          </p>
        ) : (
          <PerDayBarChart data={data.reads_per_day} />
        )}
      </CardContent>
    </Card>
  );
}

function TopSeriesCard({ data }: { data: AdminOverviewView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Most-read series — last 30 days
        </CardTitle>
        <p className="text-muted-foreground text-xs">
          Aggregated across all users. Top 10 by accumulated reading time.
        </p>
      </CardHeader>
      <CardContent>
        {data.top_series_all_users.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No reading sessions recorded yet.
          </p>
        ) : (
          <ol className="divide-border divide-y">
            {data.top_series_all_users.map((s, i) => (
              <li
                key={s.series_id}
                className="flex items-center justify-between gap-3 py-2 text-sm"
              >
                <span className="flex min-w-0 items-baseline gap-2">
                  <span className="text-muted-foreground w-6 shrink-0 text-xs tabular-nums">
                    {i + 1}.
                  </span>
                  <Link
                    href={`/series/${s.series_id}`}
                    className="text-foreground truncate font-medium hover:underline"
                  >
                    {s.name}
                  </Link>
                </span>
                <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                  {formatDurationMs(s.active_ms)} · {s.sessions}
                </span>
              </li>
            ))}
          </ol>
        )}
      </CardContent>
    </Card>
  );
}
