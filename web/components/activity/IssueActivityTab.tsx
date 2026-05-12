"use client";

import { ActivityTimeline } from "@/components/activity/ActivityTimeline";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDurationMs, formatTotalHours } from "@/lib/activity";
import { useReadingStats } from "@/lib/api/queries";

/**
 * Issue-scoped activity tab: stat cards (first read, last read, times read,
 * total time, avg sec/page) plus the per-read history table.
 */
export function IssueActivityTab({
  issueId,
  pageCount,
}: {
  issueId: string;
  /** From the issue detail row; used to compute avg sec/page when set. */
  pageCount: number | null;
}) {
  const stats = useReadingStats({ type: "issue", id: issueId }, "all");

  if (stats.isLoading) return <Skeleton className="h-48 w-full" />;
  if (!stats.data) {
    return (
      <p className="text-destructive text-sm">Failed to load issue stats.</p>
    );
  }
  const data = stats.data;
  const hasActivity = data.totals.sessions > 0;

  const avgSecPerPage =
    data.totals.distinct_pages_read > 0
      ? data.totals.active_ms / 1000 / data.totals.distinct_pages_read
      : 0;

  const cards: ReadonlyArray<{ label: string; value: string; sub?: string }> = [
    {
      label: "First read",
      value: data.first_read_at
        ? new Date(data.first_read_at).toLocaleDateString()
        : "—",
      sub: data.first_read_at
        ? new Date(data.first_read_at).toLocaleTimeString([], {
            hour: "2-digit",
            minute: "2-digit",
          })
        : "no sessions yet",
    },
    {
      label: "Last read",
      value: data.last_read_at
        ? new Date(data.last_read_at).toLocaleDateString()
        : "—",
      sub: data.last_read_at
        ? new Date(data.last_read_at).toLocaleTimeString([], {
            hour: "2-digit",
            minute: "2-digit",
          })
        : "—",
    },
    {
      label: "Times read",
      value: `${data.totals.sessions}`,
      sub: `${data.totals.distinct_pages_read} distinct pages`,
    },
    {
      label: "Total time",
      value: formatTotalHours(data.totals.active_ms / 3_600_000),
      sub: formatDurationMs(data.totals.active_ms),
    },
    {
      label: "Avg / page",
      value: avgSecPerPage > 0 ? `${Math.round(avgSecPerPage)}s` : "—",
      sub: pageCount ? `issue has ${pageCount} pages` : undefined,
    },
  ];

  return (
    <div className="space-y-6">
      <ul className="grid grid-cols-2 gap-3 md:grid-cols-5">
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
            {c.sub ? (
              <p className="text-muted-foreground mt-0.5 text-xs">{c.sub}</p>
            ) : null}
          </li>
        ))}
      </ul>

      <section>
        <h3 className="text-muted-foreground mb-2 text-xs font-semibold tracking-wide uppercase">
          Per-read history
        </h3>
        {hasActivity ? (
          <ActivityTimeline scope={{ type: "issue", id: issueId }} />
        ) : (
          <p className="text-muted-foreground text-sm">
            No sessions recorded for this issue yet.
          </p>
        )}
      </section>
    </div>
  );
}
