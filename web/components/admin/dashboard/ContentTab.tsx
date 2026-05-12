"use client";

import Link from "next/link";
import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useAdminContent } from "@/lib/api/queries";

/** Stats v2: dead-stock list + abandoned-series list + completion funnel. */
export function ContentTab() {
  const q = useAdminContent();
  if (q.isLoading) return <Skeleton className="h-64 w-full" />;
  if (q.error || !q.data) {
    return (
      <p className="text-destructive text-sm">Failed to load content stats.</p>
    );
  }
  const data = q.data;

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Completion funnel
          </CardTitle>
          <p className="text-muted-foreground text-xs">
            How far users get through issues they start. One bar per (user,
            issue) pair across the whole server.
          </p>
        </CardHeader>
        <CardContent>
          <ResponsiveContainer width="100%" height={180}>
            <BarChart
              data={data.completion_funnel.map((f) => ({
                bucket: `${f.bucket}%`,
                issues: f.issues,
              }))}
              margin={{ top: 4, right: 4, left: 0, bottom: 0 }}
            >
              <CartesianGrid
                stroke="var(--color-border)"
                strokeDasharray="2 2"
                vertical={false}
              />
              <XAxis
                dataKey="bucket"
                tick={{ fill: "var(--color-muted-foreground)", fontSize: 11 }}
                stroke="var(--color-border)"
              />
              <YAxis
                tick={{ fill: "var(--color-muted-foreground)", fontSize: 10 }}
                stroke="var(--color-border)"
                width={32}
                allowDecimals={false}
              />
              <Tooltip
                cursor={{ fill: "var(--color-muted)" }}
                contentStyle={{
                  background: "var(--color-popover)",
                  border: "1px solid var(--color-border)",
                  borderRadius: 6,
                  fontSize: 12,
                }}
              />
              <Bar
                dataKey="issues"
                fill="var(--color-primary)"
                radius={[3, 3, 0, 0]}
              />
            </BarChart>
          </ResponsiveContainer>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Abandoned series — top 20
          </CardTitle>
          <p className="text-muted-foreground text-xs">
            Series with multiple started-but-unfinished issues. Sorted by count
            of unfinished issues, then session count.
          </p>
        </CardHeader>
        <CardContent>
          {data.abandoned.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              No abandoned series — every started issue has been completed.
            </p>
          ) : (
            <ol className="divide-border divide-y">
              {data.abandoned.map((e, i) => (
                <li
                  key={e.series_id}
                  className="flex items-center justify-between gap-3 py-2 text-sm"
                >
                  <span className="flex min-w-0 items-baseline gap-2">
                    <span className="text-muted-foreground w-6 shrink-0 text-xs tabular-nums">
                      {i + 1}.
                    </span>
                    <Link
                      href={`/series/${e.series_id}`}
                      className="text-foreground truncate font-medium hover:underline"
                    >
                      {e.name}
                    </Link>
                  </span>
                  <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                    {e.unfinished_issues} unfinished · {e.sessions} session
                    {e.sessions === 1 ? "" : "s"} · {e.readers} reader
                    {e.readers === 1 ? "" : "s"}
                  </span>
                </li>
              ))}
            </ol>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Dead-stock series
          </CardTitle>
          <p className="text-muted-foreground text-xs">
            On disk but no user has opened them yet. Up to 50, sorted by issue
            count.
          </p>
        </CardHeader>
        <CardContent>
          {data.dead_stock.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              Every series with issues has at least one reader. Nice library.
            </p>
          ) : (
            <ul className="divide-border divide-y">
              {data.dead_stock.map((e) => (
                <li
                  key={e.series_id}
                  className="flex items-center justify-between gap-3 py-2 text-sm"
                >
                  <span className="flex min-w-0 items-baseline gap-2">
                    <Link
                      href={`/series/${e.series_id}`}
                      className="text-foreground truncate font-medium hover:underline"
                    >
                      {e.name}
                    </Link>
                    <span className="text-muted-foreground truncate text-xs">
                      {e.publisher ?? "—"} · {e.library_name}
                    </span>
                  </span>
                  <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                    {e.issue_count} issue{e.issue_count === 1 ? "" : "s"}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
