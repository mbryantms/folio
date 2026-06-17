"use client";

import * as React from "react";
import Link from "next/link";
import {
  ArrowLeft,
  BookOpen,
  Check,
  ChevronDown,
  ListChecks,
  MessageSquare,
  ScrollText,
  Timer,
} from "lucide-react";

import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { useReadingLogInfinite, useReadingStats } from "@/lib/api/queries";
import { formatTotalHours } from "@/lib/activity";
import { formatRelativeDate } from "@/lib/format";
import { issueUrl, seriesUrl } from "@/lib/urls";
import { statusTone } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";
import type {
  ReadingLogEventKind,
  ReadingLogEventView,
  ReadingStatsRange,
} from "@/lib/api/types";

type GroupBy = "day" | "week" | "month";

const KIND_LABEL: Record<ReadingLogEventKind, string> = {
  issue_finished: "Finished",
  series_finished: "Series finished",
  session_completed: "Reading session",
  marker_created: "Bookmark",
};
const KIND_ICON: Record<ReadingLogEventKind, typeof Check> = {
  issue_finished: Check,
  series_finished: ListChecks,
  session_completed: Timer,
  marker_created: MessageSquare,
};
const KIND_TINT: Record<ReadingLogEventKind, string> = {
  issue_finished: statusTone("success"),
  series_finished: "bg-primary/15 text-primary",
  session_completed: statusTone("info"),
  marker_created: statusTone("warning"),
};

function rangeToFrom(range: ReadingStatsRange): string | undefined {
  if (range === "all") return undefined;
  const days =
    range === "7d"
      ? 7
      : range === "30d"
        ? 30
        : range === "60d"
          ? 60
          : range === "90d"
            ? 90
            : 365;
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() - days);
  return cutoff.toISOString();
}

/** Full activity report — the rich counterpart to the `/log` page's
 *  Activity widget. Reached by clicking the widget's title.
 *  Headline stats sit at the top; below, every event in the
 *  selected range renders as a large cover-first card grid grouped
 *  by day / week / month. */
export function ActivityReportPage() {
  const [range, setRange] = React.useState<ReadingStatsRange>("30d");
  const [groupBy, setGroupBy] = React.useState<GroupBy>("week");
  const stats = useReadingStats({ type: "all" }, range);
  const query = useReadingLogInfinite(
    React.useMemo(
      () => ({
        kinds: ["issue_finished"],
        from: rangeToFrom(range),
        limit: 60,
      }),
      [range],
    ),
  );

  // Sentinel near the bottom triggers the next page fetch as the
  // user scrolls — the report can grow very long, so we lazy-load
  // even at limit 60 per page.
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  // Depend on the three fields, not the whole query object (audit G10) —
  // `[query]` rebuilt the observer every render.
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  React.useEffect(() => {
    const node = sentinelRef.current;
    if (!node) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (
          entries.some((e) => e.isIntersecting) &&
          hasNextPage &&
          !isFetchingNextPage
        ) {
          void fetchNextPage();
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(node);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  const events = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.events) ?? [],
    [query.data],
  );
  const groups = React.useMemo(
    () => buildGroups(events, groupBy),
    [events, groupBy],
  );

  return (
    <div className="space-y-6">
      <ReportHeader
        range={range}
        onRangeChange={setRange}
        groupBy={groupBy}
        onGroupByChange={setGroupBy}
      />
      <StatsRow stats={stats.data} loading={stats.isLoading} />
      {query.isLoading ? (
        <FeedSkeleton />
      ) : events.length === 0 ? (
        <EmptyState range={range} />
      ) : (
        <div className="flex flex-col gap-10">
          {groups.map((g) => (
            <section key={g.key} aria-label={g.label}>
              <SectionHeader
                label={g.label}
                subtitle={g.subtitle}
                count={g.events.length}
              />
              <div className="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-4 sm:grid-cols-[repeat(auto-fill,minmax(220px,1fr))]">
                {g.events.map((e) => (
                  <EventCard key={e.id} event={e} />
                ))}
              </div>
            </section>
          ))}
        </div>
      )}
      <div ref={sentinelRef} aria-hidden className="h-px" />
      {query.isFetchingNextPage && (
        <div className="text-muted-foreground flex justify-center gap-1 text-sm">
          <ChevronDown className="h-4 w-4 animate-pulse" />
          Loading more…
        </div>
      )}
      {!query.hasNextPage && events.length > 0 && (
        <p className="text-muted-foreground/70 text-center text-xs">
          End of activity for this range.
        </p>
      )}
    </div>
  );
}

function ReportHeader({
  range,
  onRangeChange,
  groupBy,
  onGroupByChange,
}: {
  range: ReadingStatsRange;
  onRangeChange: (next: ReadingStatsRange) => void;
  groupBy: GroupBy;
  onGroupByChange: (next: GroupBy) => void;
}) {
  return (
    <header className="flex flex-col gap-3">
      <Button asChild variant="ghost" size="sm" className="-ml-2 w-fit">
        <Link href="/log">
          <ArrowLeft aria-hidden="true" className="mr-1 h-4 w-4" />
          Back to reading log
        </Link>
      </Button>
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="flex items-center gap-2.5">
          <ScrollText className="text-muted-foreground h-5 w-5" />
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">
              Activity report
            </h1>
            <p className="text-muted-foreground text-sm">
              Everything you&rsquo;ve finished in the selected window — full
              covers, credits, and read dates.
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ActivityRangeSelector value={range} onChange={onRangeChange} />
          <Select
            value={groupBy}
            onValueChange={(v) => onGroupByChange(v as GroupBy)}
          >
            <SelectTrigger className="w-32" aria-label="Group events by">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="day">By day</SelectItem>
              <SelectItem value="week">By week</SelectItem>
              <SelectItem value="month">By month</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
    </header>
  );
}

function StatsRow({
  stats,
  loading,
}: {
  stats: ReturnType<typeof useReadingStats>["data"];
  loading: boolean;
}) {
  if (loading) {
    return (
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-24" />
        ))}
      </div>
    );
  }
  if (!stats) return null;
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      <StatTile
        label="Issues finished"
        value={stats.totals.distinct_issues.toLocaleString()}
        Icon={BookOpen}
      />
      <StatTile
        label="Time read"
        value={formatTotalHours(stats.totals.active_ms / 3_600_000)}
        Icon={Timer}
      />
      <StatTile
        label="Pages read"
        value={stats.totals.distinct_pages_read.toLocaleString()}
        Icon={ScrollText}
      />
      <StatTile
        label="Current streak"
        value={`${stats.totals.current_streak}d`}
        Icon={Check}
      />
    </div>
  );
}

function StatTile({
  label,
  value,
  Icon,
}: {
  label: string;
  value: string;
  Icon: typeof BookOpen;
}) {
  return (
    <Card>
      <CardContent className="flex flex-col gap-1.5 p-4">
        <Icon aria-hidden="true" className="text-muted-foreground h-4 w-4" />
        <div className="text-2xl leading-tight font-semibold tabular-nums">
          {value}
        </div>
        <div className="text-muted-foreground text-[11px] tracking-wider uppercase">
          {label}
        </div>
      </CardContent>
    </Card>
  );
}

function SectionHeader({
  label,
  subtitle,
  count,
}: {
  label: string;
  subtitle: string | null;
  count: number;
}) {
  return (
    <div className="border-border/40 mb-4 flex flex-wrap items-baseline justify-between gap-2 border-b pb-2">
      <div>
        <h2 className="text-2xl font-semibold tracking-tight">{label}</h2>
        {subtitle ? (
          <p className="text-muted-foreground text-sm">{subtitle}</p>
        ) : null}
      </div>
      <p className="text-muted-foreground text-sm">
        {count} issue{count === 1 ? "" : "s"}
      </p>
    </div>
  );
}

function EventCard({ event }: { event: ReadingLogEventView }) {
  const Icon = KIND_ICON[event.kind as ReadingLogEventKind];
  const cover = event.issue?.cover_url ?? event.series?.cover_url ?? null;
  const seriesName = event.series?.name ?? "—";
  const number = event.issue?.number ?? null;
  const writers = event.issue?.writer ?? null;
  const pageCount = event.issue?.page_count ?? null;
  const readAt = event.occurred_at;

  const href =
    event.series && event.issue
      ? issueUrl(event.series.slug, event.issue.slug)
      : event.series
        ? seriesUrl(event.series.slug)
        : null;

  const card = (
    <Card className="group/card hover:border-primary/40 flex h-full flex-col overflow-hidden transition-colors">
      <div className="bg-muted relative aspect-2/3 w-full overflow-hidden">
        {cover ? (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={cover}
            alt={`${seriesName} ${number ? `#${number}` : ""} cover`}
            className="h-full w-full object-cover transition-transform duration-200 group-hover/card:scale-[1.02]"
            loading="lazy"
          />
        ) : (
          <div className="text-muted-foreground/60 flex h-full w-full items-center justify-center text-xs">
            No cover
          </div>
        )}
        <span
          className={cn(
            "absolute top-2 left-2 inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium tracking-wide uppercase backdrop-blur",
            KIND_TINT[event.kind as ReadingLogEventKind],
          )}
        >
          <Icon aria-hidden="true" className="h-3.5 w-3.5" />
          {KIND_LABEL[event.kind as ReadingLogEventKind]}
        </span>
      </div>
      <CardContent className="flex flex-1 flex-col gap-1.5 p-3.5">
        <div className="line-clamp-2 text-base leading-tight font-semibold">
          {seriesName}
        </div>
        {number ? (
          <div className="text-primary text-sm font-medium tabular-nums">
            #{number}
          </div>
        ) : null}
        {writers ? (
          <p
            className="text-muted-foreground line-clamp-2 text-xs italic"
            title={writers}
          >
            by {writers}
          </p>
        ) : null}
        <div className="mt-auto pt-1.5">
          {pageCount != null ? (
            <p className="text-muted-foreground/80 text-xs">
              {pageCount} page{pageCount === 1 ? "" : "s"}
            </p>
          ) : null}
          <p
            className="text-muted-foreground/70 text-xs"
            title={new Date(readAt).toLocaleString()}
          >
            Read: {formatRelativeDate(readAt)}
          </p>
        </div>
      </CardContent>
    </Card>
  );

  if (href) {
    return (
      <Link href={href} className="block focus:outline-none">
        {card}
      </Link>
    );
  }
  return card;
}

// ─── Grouping ───

type Group = {
  key: string;
  label: string;
  subtitle: string | null;
  events: ReadingLogEventView[];
};

function periodMeta(
  iso: string,
  groupBy: GroupBy,
): { key: string; label: string; subtitle: string | null } {
  const d = new Date(iso);
  if (groupBy === "month") {
    return {
      key: `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`,
      label: d.toLocaleDateString(undefined, {
        month: "long",
        year: "numeric",
      }),
      subtitle: null,
    };
  }
  if (groupBy === "week") {
    const monday = new Date(d);
    const dow = monday.getDay();
    const diff = (dow + 6) % 7;
    monday.setDate(monday.getDate() - diff);
    monday.setHours(0, 0, 0, 0);
    const sunday = new Date(monday);
    sunday.setDate(monday.getDate() + 6);
    const fmtShort = (x: Date) =>
      x.toLocaleDateString(undefined, { month: "short", day: "numeric" });
    return {
      key: `wk-${monday.toISOString().slice(0, 10)}`,
      label: `Week of ${fmtShort(monday)}`,
      subtitle: `${fmtShort(monday)} — ${fmtShort(sunday)}`,
    };
  }
  return {
    key: d.toLocaleDateString(undefined, {
      year: "numeric",
      month: "numeric",
      day: "numeric",
    }),
    label: d.toLocaleDateString(undefined, {
      weekday: "long",
      month: "short",
      day: "numeric",
    }),
    subtitle: null,
  };
}

function buildGroups(events: ReadingLogEventView[], groupBy: GroupBy): Group[] {
  const groups: Group[] = [];
  for (const e of events) {
    const meta = periodMeta(e.occurred_at, groupBy);
    const last = groups[groups.length - 1];
    if (last && last.key === meta.key) {
      last.events.push(e);
    } else {
      groups.push({ ...meta, events: [e] });
    }
  }
  return groups;
}

function FeedSkeleton() {
  return (
    <div className="flex flex-col gap-10">
      {Array.from({ length: 2 }).map((_, i) => (
        <section key={i}>
          <Skeleton className="mb-4 h-8 w-48" />
          <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4">
            {Array.from({ length: 6 }).map((_, j) => (
              <Skeleton key={j} className="aspect-2/3.5 w-full" />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

function EmptyState({ range }: { range: ReadingStatsRange }) {
  return (
    <div className="border-border/60 text-muted-foreground flex flex-col items-center gap-3 rounded-md border border-dashed px-6 py-16 text-center">
      <BookOpen className="text-muted-foreground/40 h-10 w-10" />
      <p className="text-base">
        No issues finished in the{" "}
        {range === "all" ? "entire history" : `last ${range}`}.
      </p>
      <p className="text-muted-foreground/80 max-w-sm text-sm">
        Mark an issue read from any series page, or finish one in the reader —
        it&rsquo;ll show up here.
      </p>
    </div>
  );
}
