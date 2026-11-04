"use client";

import { CountUp } from "@/components/CountUp";
import type { ReadingStatsView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/**
 * The Stats tab's headline band (reading log, folio-flair phase 4): six
 * large tabular figures in one card, split by hairlines. Every number
 * counts up on first view (`CountUp`; reduced motion shows the final
 * value). Totals are scoped to the selected range; the streak is global.
 */
export function HeroCards({ data }: { data: ReadingStatsView }) {
  const { totals, completion } = data;
  const totalMinutes = Math.round(totals.active_ms / 60_000);
  const perDayMinutes =
    totals.days_active > 0 ? Math.round(totalMinutes / totals.days_active) : 0;
  const pagesPerDay =
    totals.days_active > 0
      ? Math.round(totals.distinct_pages_read / totals.days_active)
      : 0;

  const tiles: ReadonlyArray<Tile> = [
    {
      label: "Time read",
      figure: <Duration minutes={totalMinutes} />,
      sub: plural(totals.sessions, "session"),
    },
    {
      label: "Issues",
      figure: <CountUp value={totals.distinct_issues} />,
      sub: `${completion.completed.toLocaleString()} finished`,
    },
    {
      label: "Pages",
      figure: <CountUp value={totals.distinct_pages_read} />,
      sub:
        totals.days_active > 0 ? `~${pagesPerDay} per reading day` : "none yet",
    },
    {
      label: "Reading days",
      figure: <CountUp value={totals.days_active} />,
      sub:
        totals.days_active > 0
          ? `~${formatMinutes(perDayMinutes)} a day`
          : "none yet",
    },
    {
      label: "Completion",
      figure: (
        <CountUp value={Math.round(completion.rate * 100)} format="percent" />
      ),
      sub: `${completion.completed.toLocaleString()} of ${plural(completion.started, "issue")} started`,
    },
    {
      label: "Streak",
      figure: (
        <>
          <CountUp value={totals.current_streak} />
          <Unit>{totals.current_streak === 1 ? "day" : "days"}</Unit>
        </>
      ),
      sub:
        totals.longest_streak > 0 &&
        totals.longest_streak === totals.current_streak
          ? "your longest yet"
          : `longest ${plural(totals.longest_streak, "day")}`,
    },
  ];

  return (
    <ul className="border-border bg-border grid grid-cols-2 gap-px overflow-hidden rounded-xl border sm:grid-cols-3 xl:grid-cols-6">
      {tiles.map((t) => (
        <li key={t.label} className="bg-card min-w-0 px-4 py-4 sm:px-5 sm:py-5">
          <p className="text-muted-foreground truncate text-xs font-medium tracking-wide uppercase">
            {t.label}
          </p>
          <p className="text-foreground mt-2 flex items-baseline gap-1 truncate text-3xl font-semibold tracking-tight tabular-nums">
            {t.figure}
          </p>
          <p className="text-muted-foreground mt-1 truncate text-xs">{t.sub}</p>
        </li>
      ))}
    </ul>
  );
}

type Tile = { label: string; figure: React.ReactNode; sub: string };

function Unit({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "text-muted-foreground text-sm font-medium tracking-normal",
        className,
      )}
    >
      {children}
    </span>
  );
}

/** `3h 25m` / `42m`, each part counting up on its own. */
function Duration({ minutes }: { minutes: number }) {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) {
    return (
      <>
        <CountUp value={m} />
        <Unit>m</Unit>
      </>
    );
  }
  return (
    <>
      <CountUp value={h} />
      <Unit>h</Unit>
      {m > 0 ? (
        <>
          <CountUp value={m} className="ml-1" />
          <Unit>m</Unit>
        </>
      ) : null}
    </>
  );
}

function formatMinutes(minutes: number): string {
  if (minutes < 60) return `${minutes}m`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m > 0 ? `${h}h ${m}m` : `${h}h`;
}

function plural(n: number, word: string): string {
  return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
}
