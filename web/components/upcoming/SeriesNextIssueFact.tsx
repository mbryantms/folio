"use client";

import Link from "next/link";
import { CalendarClock } from "lucide-react";

import { UpcomingStageBadge } from "@/components/upcoming/UpcomingStageBadge";
import { useSeriesNextRelease } from "@/lib/api/queries";
import { releaseDay } from "@/lib/upcoming/stage";
import { useLocalToday } from "@/lib/upcoming/today";

/**
 * "Next issue #27 · Wed 25 Nov" in the series page's fact row, with the
 * stage badge when someone tracks it (upcoming-releases 1.0). Renders
 * nothing while the feature is off, when no issue is announced, or when
 * the next announced issues are already in the library.
 *
 * A fact, not a control: it links to the Upcoming page and does nothing
 * else.
 */
export function SeriesNextIssueFact({ slug }: { slug: string }) {
  const today = useLocalToday();
  const q = useSeriesNextRelease(slug, today ?? "", {
    enabled: today !== null,
  });
  const next = q.data?.entries.find((e) => !e.owned);
  if (!next || today === null) return null;
  return (
    <span
      className="inline-flex items-center gap-1.5"
      data-testid="series-next-issue"
    >
      <span className="text-muted-foreground/80 hidden sm:inline">
        <CalendarClock className="size-4" aria-hidden="true" />
      </span>
      <Link
        href="/upcoming"
        className="hover:text-foreground underline-offset-4 hover:underline"
      >
        Next issue #{next.number} · {releaseDay(next.release_date, today)}
      </Link>
      {next.stage === "announced" ? null : <UpcomingStageBadge entry={next} />}
    </span>
  );
}
