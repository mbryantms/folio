"use client";

import { Flame, History, Library, Sparkles, Target, Timer } from "lucide-react";

import { formatTotalHours } from "@/lib/activity";
import type { ReadingStatsView } from "@/lib/api/types";

/**
 * Six oversized stat tiles that anchor the /settings/activity page. Each
 * shows a single headline number plus a small sub-label. Icons reuse the
 * accent palette so the row reads as one design system unit.
 */
export function HeroCards({ data }: { data: ReadingStatsView }) {
  const totalHours = data.totals.active_ms / 3_600_000;
  const completionPct = Math.round(data.completion.rate * 100);

  const tiles: ReadonlyArray<Tile> = [
    {
      label: "Time read",
      value: formatTotalHours(totalHours),
      sub: `${data.totals.sessions.toLocaleString()} session${data.totals.sessions === 1 ? "" : "s"}`,
      Icon: Timer,
    },
    {
      label: "Issues read",
      value: data.totals.distinct_issues.toLocaleString(),
      sub: `${data.totals.distinct_pages_read.toLocaleString()} pages`,
      Icon: Library,
    },
    {
      label: "Series touched",
      value: `${data.reread_top_series.length || data.top_series.length}`,
      sub: data.reread_top_series[0]?.name ?? data.top_series[0]?.name ?? "—",
      Icon: Sparkles,
    },
    {
      label: "Completion",
      value: `${completionPct}%`,
      sub: `${data.completion.completed.toLocaleString()} / ${data.completion.started.toLocaleString()} issues`,
      Icon: Target,
    },
    {
      label: "Current streak",
      value: `${data.totals.current_streak}d`,
      sub: data.totals.current_streak > 0 ? "keep it going" : "no streak today",
      Icon: Flame,
    },
    {
      label: "Longest streak",
      value: `${data.totals.longest_streak}d`,
      sub:
        data.totals.longest_streak === data.totals.current_streak &&
        data.totals.current_streak > 0
          ? "personal best — right now"
          : "personal best",
      Icon: History,
    },
  ];

  return (
    <ul className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
      {tiles.map((t) => (
        <li
          key={t.label}
          className="border-border bg-card relative overflow-hidden rounded-lg border p-4"
        >
          <div className="text-muted-foreground flex items-center justify-between text-xs font-medium tracking-wide uppercase">
            <span>{t.label}</span>
            <t.Icon className="size-3.5 opacity-70" aria-hidden />
          </div>
          <p className="text-foreground mt-2 text-2xl font-semibold tabular-nums">
            {t.value}
          </p>
          <p className="text-muted-foreground mt-1 truncate text-xs">{t.sub}</p>
        </li>
      ))}
    </ul>
  );
}

type Tile = {
  label: string;
  value: string;
  sub: string;
  Icon: React.ComponentType<{ className?: string }>;
};
