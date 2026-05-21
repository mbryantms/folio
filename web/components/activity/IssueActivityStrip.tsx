"use client";

import { formatDurationMs, formatTotalHours } from "@/lib/activity";
import type { ReadingStatsView } from "@/lib/api/types";

/**
 * Headline activity cards shown at the top of the issue Activity tab.
 * Mirrors the un-labelled `StatCards` grid on the series Activity tab so
 * both pages introduce the timeline with the same visual rhythm.
 */
export function IssueActivityStrip({
  stats,
  pageCount,
}: {
  stats: ReadingStatsView;
  /** Used to caption the avg-per-page card with the issue's length. */
  pageCount: number | null;
}) {
  const totals = stats.totals;
  const avgSecPerPage =
    totals.distinct_pages_read > 0
      ? totals.active_ms / 1000 / totals.distinct_pages_read
      : 0;

  const cards: ReadonlyArray<{ label: string; value: string; sub?: string }> = [
    {
      label: "First read",
      value: stats.first_read_at
        ? new Date(stats.first_read_at).toLocaleDateString()
        : "—",
      sub: stats.first_read_at
        ? new Date(stats.first_read_at).toLocaleTimeString([], {
            hour: "2-digit",
            minute: "2-digit",
          })
        : undefined,
    },
    {
      label: "Last read",
      value: stats.last_read_at
        ? new Date(stats.last_read_at).toLocaleDateString()
        : "—",
      sub: stats.last_read_at
        ? new Date(stats.last_read_at).toLocaleTimeString([], {
            hour: "2-digit",
            minute: "2-digit",
          })
        : undefined,
    },
    {
      label: "Times read",
      value: `${totals.sessions}`,
      sub: `${totals.distinct_pages_read} distinct pages`,
    },
    {
      label: "Total time",
      value: formatTotalHours(totals.active_ms / 3_600_000),
      sub: formatDurationMs(totals.active_ms),
    },
    {
      label: "Avg / page",
      value: avgSecPerPage > 0 ? `${Math.round(avgSecPerPage)}s` : "—",
      sub: pageCount ? `issue has ${pageCount} pages` : undefined,
    },
  ];

  return (
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
  );
}
