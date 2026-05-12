"use client";

import Link from "next/link";

import { formatDurationMs } from "@/lib/activity";
import type { RereadIssueEntry, RereadSeriesEntry } from "@/lib/api/types";

/**
 * Most-reread issues + series side by side. Empty when no issue or series
 * has been read more than once.
 */
export function RereadList({
  issues,
  series,
}: {
  issues: ReadonlyArray<RereadIssueEntry>;
  series: ReadonlyArray<RereadSeriesEntry>;
}) {
  const topIssues = issues.filter((i) => i.reads > 1).slice(0, 10);
  const topSeries = series.filter((s) => s.reads > 1).slice(0, 10);

  if (topIssues.length === 0 && topSeries.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        Rereads show up once you&apos;ve revisited an issue or series.
      </p>
    );
  }

  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
      {topIssues.length > 0 ? (
        <section className="border-border bg-card rounded-md border p-4">
          <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
            Most reread issues
          </h3>
          <ol className="divide-border divide-y">
            {topIssues.map((entry, i) => (
              <li
                key={entry.issue_id}
                className="flex items-center justify-between gap-3 py-1.5 text-sm"
              >
                <span className="flex min-w-0 items-baseline gap-2">
                  <span className="text-muted-foreground w-5 shrink-0 text-xs tabular-nums">
                    {i + 1}.
                  </span>
                  <Link
                    href={`/series/${entry.series_id}`}
                    className="text-foreground truncate font-medium hover:underline"
                  >
                    <span className="text-muted-foreground">
                      {entry.series_name}
                      {entry.number_raw ? ` #${entry.number_raw}` : ""}{" "}
                    </span>
                    {entry.title ?? "Untitled"}
                  </Link>
                </span>
                <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                  ×{entry.reads} · {formatDurationMs(entry.active_ms)}
                </span>
              </li>
            ))}
          </ol>
        </section>
      ) : null}
      {topSeries.length > 0 ? (
        <section className="border-border bg-card rounded-md border p-4">
          <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
            Most reread series
          </h3>
          <ol className="divide-border divide-y">
            {topSeries.map((entry, i) => (
              <li
                key={entry.series_id}
                className="flex items-center justify-between gap-3 py-1.5 text-sm"
              >
                <span className="flex min-w-0 items-baseline gap-2">
                  <span className="text-muted-foreground w-5 shrink-0 text-xs tabular-nums">
                    {i + 1}.
                  </span>
                  <Link
                    href={`/series/${entry.series_id}`}
                    className="text-foreground truncate font-medium hover:underline"
                  >
                    {entry.name}
                  </Link>
                </span>
                <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                  ×{entry.reads} · {entry.distinct_issues} issue
                  {entry.distinct_issues === 1 ? "" : "s"}
                </span>
              </li>
            ))}
          </ol>
        </section>
      ) : null}
    </div>
  );
}
