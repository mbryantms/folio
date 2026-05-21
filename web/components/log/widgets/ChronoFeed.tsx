"use client";

import * as React from "react";
import Link from "next/link";
import {
  BookmarkIcon,
  Check,
  ChevronDown,
  ListChecks,
  MessageSquare,
  Star,
  Timer,
} from "lucide-react";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingLogInfinite } from "@/lib/api/queries";
import { formatDurationMs } from "@/lib/activity";
import { formatRelativeDate } from "@/lib/format";
import { issueUrl, seriesUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";
import type {
  ReadingLogEventKind,
  ReadingLogEventView,
  ReadingLogFilters,
  ReadingLogPayload,
  ReadingStatsRange,
} from "@/lib/api/types";

import { WidgetCard } from "../WidgetCard";
import type {
  ChronoFeedConfig,
  ChronoFeedGroupBy,
  LogWidgetProps,
} from "./types";

const KIND_ICON: Record<ReadingLogEventKind, typeof Check> = {
  issue_finished: Check,
  series_finished: ListChecks,
  session_completed: Timer,
  marker_created: MessageSquare,
};

const KIND_LABEL: Record<ReadingLogEventKind, string> = {
  issue_finished: "Finished",
  series_finished: "Series finished",
  session_completed: "Reading session",
  marker_created: "Bookmark",
};

const KIND_TINT: Record<ReadingLogEventKind, string> = {
  issue_finished: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300",
  series_finished: "bg-primary/15 text-primary",
  session_completed: "bg-sky-500/15 text-sky-700 dark:text-sky-300",
  marker_created: "bg-amber-500/15 text-amber-700 dark:text-amber-300",
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

/** Reverse-chronological feed of every reading-activity event the
 *  user has produced. Cursor-paginated; an IntersectionObserver
 *  sentinel at the tail auto-loads the next page on scroll inside
 *  the bounded-height container.
 *
 *  Grouping is configurable (`day` / `week` / `month` / `none`).
 *  Within a group, consecutive `issue_finished` events for the same
 *  series collapse into a single summary row — a 12-issue arc on a
 *  Saturday shows as one line ("X-Men: Legacy · #5–#16, 12 issues")
 *  rather than twelve repeating rows. */
export function ChronoFeed({
  widget,
  scope,
}: LogWidgetProps<ChronoFeedConfig>) {
  const groupBy: ChronoFeedGroupBy = widget.config.group_by ?? "day";
  // Empty-string `range` is the sentinel for "follow the page-level
  // range selector"; anything else is an explicit per-widget
  // override and wins.
  const configuredRange = widget.config.range;
  const effectiveRange: ReadingStatsRange =
    configuredRange && configuredRange.length > 0
      ? (configuredRange as ReadingStatsRange)
      : scope.range;
  const filters: ReadingLogFilters = React.useMemo(() => {
    const widgetKinds = widget.config.default_kinds ?? [];
    const kinds = widgetKinds.length > 0 ? widgetKinds : undefined;
    return {
      kinds,
      from: rangeToFrom(effectiveRange),
      limit: 30,
    };
  }, [effectiveRange, widget.config.default_kinds]);

  const query = useReadingLogInfinite(filters);
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  React.useEffect(() => {
    const node = sentinelRef.current;
    const root = scrollRef.current;
    if (!node || !root) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (
          entries.some((e) => e.isIntersecting) &&
          query.hasNextPage &&
          !query.isFetchingNextPage
        ) {
          void query.fetchNextPage();
        }
      },
      { root, rootMargin: "120px" },
    );
    obs.observe(node);
    return () => obs.disconnect();
  }, [query]);

  const events: ReadingLogEventView[] = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.events) ?? [],
    [query.data],
  );

  const groups = React.useMemo(
    () => buildGroups(events, groupBy),
    [events, groupBy],
  );

  return (
    <WidgetCard widget={widget} title="Activity" titleHref="/log/activity">
      <div ref={scrollRef} className="max-h-160 overflow-y-auto pr-1">
        {query.isLoading ? (
          <FeedSkeleton />
        ) : events.length === 0 ? (
          <EmptyState />
        ) : (
          <ol className="flex flex-col gap-5">
            {groups.map((g) => (
              <li key={g.key} className="flex flex-col gap-2">
                {g.label ? <GroupHeader label={g.label} /> : null}
                <ul
                  className={cn(
                    "flex flex-col gap-2",
                    g.label && "border-border/60 border-l-2 pl-4",
                  )}
                >
                  {g.rows.map((row) => (
                    <li key={row.key}>
                      {row.kind === "single" ? (
                        <EventRow event={row.event} />
                      ) : (
                        <SeriesRollupRow rollup={row} />
                      )}
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ol>
        )}
        <div ref={sentinelRef} aria-hidden className="h-px" />
        {query.isFetchingNextPage && (
          <div className="text-muted-foreground mt-4 flex justify-center text-xs">
            <ChevronDown className="mr-1 h-3 w-3 animate-pulse" />
            Loading more…
          </div>
        )}
        {!query.hasNextPage && events.length > 0 && (
          <p className="text-muted-foreground/70 mt-6 text-center text-xs">
            That&rsquo;s everything in this range.
          </p>
        )}
      </div>
    </WidgetCard>
  );
}

// ─── Grouping + collapsing ───

type RowSingle = { kind: "single"; key: string; event: ReadingLogEventView };
type RowRollup = {
  kind: "rollup";
  key: string;
  seriesId: string;
  seriesName: string;
  seriesSlug: string;
  /** Earliest cover among the collapsed events — first one we see
   *  walking reverse-chronologically. */
  coverUrl: string | null;
  /** Issue numbers in original order (most-recent first). */
  issueNumbers: string[];
  /** Most-recent occurrence; drives the relative timestamp. */
  latestOccurredAt: string;
  earliestOccurredAt: string;
};
type Row = RowSingle | RowRollup;

type Section = {
  key: string;
  /** `null` for the flat (no-group) layout. */
  label: string | null;
  rows: Row[];
};

/** Group key per period. Strings sort lexicographically the same as
 *  the corresponding date does, so we can compare them directly when
 *  walking events in chrono-DESC order. */
function periodKey(iso: string, groupBy: ChronoFeedGroupBy): string {
  if (groupBy === "none") return "all";
  const d = new Date(iso);
  if (groupBy === "month") {
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
  }
  if (groupBy === "week") {
    // ISO week-of-year would be nicer; rolling 7-day buckets anchored
    // to Monday are good enough for the rendering grouping.
    const monday = new Date(d);
    const day = monday.getDay();
    const diff = (day + 6) % 7; // 0 = Mon, 6 = Sun
    monday.setDate(monday.getDate() - diff);
    monday.setHours(0, 0, 0, 0);
    return `wk-${monday.toISOString().slice(0, 10)}`;
  }
  // `day`
  return d.toLocaleDateString(undefined, {
    year: "numeric",
    month: "numeric",
    day: "numeric",
  });
}

function periodLabel(iso: string, groupBy: ChronoFeedGroupBy): string {
  if (groupBy === "none") return "";
  const d = new Date(iso);
  if (groupBy === "month") {
    return d.toLocaleDateString(undefined, { month: "long", year: "numeric" });
  }
  if (groupBy === "week") {
    const monday = new Date(d);
    const day = monday.getDay();
    const diff = (day + 6) % 7;
    monday.setDate(monday.getDate() - diff);
    return `Week of ${monday.toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
    })}`;
  }
  return d.toLocaleDateString(undefined, {
    weekday: "long",
    month: "short",
    day: "numeric",
  });
}

function buildGroups(
  events: ReadingLogEventView[],
  groupBy: ChronoFeedGroupBy,
): Section[] {
  const sections: Section[] = [];
  for (const e of events) {
    const key = periodKey(e.occurred_at, groupBy);
    let section = sections[sections.length - 1];
    if (!section || section.key !== key) {
      section = {
        key,
        label: groupBy === "none" ? null : periodLabel(e.occurred_at, groupBy),
        rows: [],
      };
      sections.push(section);
    }
    // Collapse adjacent issue_finished rows from the same series
    // into a single rollup row. Other kinds (series_finished,
    // session_completed, marker_created) always render as singles.
    const lastRow = section.rows[section.rows.length - 1];
    const isIssueFinished = e.kind === "issue_finished";
    const sid = e.series?.id ?? null;
    if (
      isIssueFinished &&
      sid &&
      lastRow &&
      lastRow.kind === "rollup" &&
      lastRow.seriesId === sid
    ) {
      lastRow.issueNumbers.push(e.issue?.number ?? "?");
      lastRow.earliestOccurredAt = e.occurred_at;
      continue;
    }
    if (
      isIssueFinished &&
      sid &&
      lastRow &&
      lastRow.kind === "single" &&
      lastRow.event.kind === "issue_finished" &&
      lastRow.event.series?.id === sid
    ) {
      // Promote the prior single to a rollup so future siblings can
      // collapse into it too. Carry the prior event's number first.
      const prev = lastRow.event;
      section.rows[section.rows.length - 1] = {
        kind: "rollup",
        key: `rollup-${sid}-${prev.id}`,
        seriesId: sid,
        seriesName: prev.series?.name ?? "Series",
        seriesSlug: prev.series?.slug ?? "",
        coverUrl: prev.issue?.cover_url ?? null,
        issueNumbers: [prev.issue?.number ?? "?", e.issue?.number ?? "?"],
        latestOccurredAt: prev.occurred_at,
        earliestOccurredAt: e.occurred_at,
      } satisfies RowRollup;
      continue;
    }
    section.rows.push({
      kind: "single",
      key: e.id,
      event: e,
    });
  }
  return sections;
}

// ─── Row renderers ───

function GroupHeader({ label }: { label: string }) {
  return (
    <div className="text-foreground border-border/40 border-b pb-1.5 text-lg font-semibold tracking-tight">
      {label}
    </div>
  );
}

function EventRow({ event }: { event: ReadingLogEventView }) {
  const Icon = KIND_ICON[event.kind];
  const cover = event.issue?.cover_url ?? event.series?.cover_url ?? null;
  const issueLabel = event.issue?.number ? `#${event.issue.number}` : null;
  const issueTitle = event.issue?.title ?? null;
  const seriesName = event.series?.name ?? null;
  const headline = issueTitle ?? seriesName ?? "Reading event";

  const inner = (
    <div className="hover:bg-muted/50 group/event flex gap-3.5 rounded-md p-2 transition-colors">
      <div
        className={cn(
          "border-border/60 relative aspect-2/3 w-14 shrink-0 overflow-hidden rounded border",
          !cover && "bg-muted",
        )}
        aria-hidden
      >
        {cover ? (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={cover}
            alt=""
            className="h-full w-full object-cover"
            loading="lazy"
          />
        ) : null}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex flex-wrap items-center gap-1.5">
          <span
            className={cn(
              "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium tracking-wide uppercase",
              KIND_TINT[event.kind],
            )}
          >
            <Icon aria-hidden="true" className="h-3.5 w-3.5" />
            {KIND_LABEL[event.kind]}
          </span>
          {issueLabel ? (
            <span className="text-muted-foreground text-sm font-medium tabular-nums">
              {issueLabel}
            </span>
          ) : null}
          <time
            className="text-muted-foreground/60 ml-auto text-[10px]"
            title={new Date(event.occurred_at).toLocaleString()}
          >
            {formatRelativeDate(event.occurred_at)}
          </time>
        </div>
        <div className="truncate text-base font-medium" title={headline}>
          {headline}
        </div>
        <PayloadLine event={event} />
      </div>
    </div>
  );

  if (event.kind === "series_finished" && event.series) {
    return <Link href={seriesUrl(event.series.slug)}>{inner}</Link>;
  }
  if (event.issue && event.series) {
    return (
      <Link href={issueUrl(event.series.slug, event.issue.slug)}>{inner}</Link>
    );
  }
  return inner;
}

function SeriesRollupRow({ rollup }: { rollup: RowRollup }) {
  const inner = (
    <div className="hover:bg-muted/50 group/event flex gap-3.5 rounded-md p-2 transition-colors">
      <div
        className={cn(
          "border-border/60 relative aspect-2/3 w-14 shrink-0 overflow-hidden rounded border",
          !rollup.coverUrl && "bg-muted",
        )}
        aria-hidden
      >
        {rollup.coverUrl ? (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={rollup.coverUrl}
            alt=""
            className="h-full w-full object-cover"
            loading="lazy"
          />
        ) : null}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex flex-wrap items-center gap-1.5">
          <span
            className={cn(
              "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium tracking-wide uppercase",
              KIND_TINT.issue_finished,
            )}
          >
            <Check aria-hidden="true" className="h-3.5 w-3.5" />
            {rollup.issueNumbers.length} finished
          </span>
          <time
            className="text-muted-foreground/60 ml-auto text-[10px]"
            title={new Date(rollup.latestOccurredAt).toLocaleString()}
          >
            {formatRelativeDate(rollup.latestOccurredAt)}
          </time>
        </div>
        <div
          className="truncate text-base font-medium"
          title={rollup.seriesName}
        >
          {rollup.seriesName}
        </div>
        <p
          className="text-muted-foreground truncate text-xs"
          title={rollup.issueNumbers.map((n) => `#${n}`).join(", ")}
        >
          {summarizeIssueNumbers(rollup.issueNumbers)}
        </p>
      </div>
    </div>
  );
  return rollup.seriesSlug ? (
    <Link href={seriesUrl(rollup.seriesSlug)}>{inner}</Link>
  ) : (
    inner
  );
}

/** Render a list of issue numbers as a tight range when contiguous,
 *  otherwise a comma-separated list capped to a few entries. */
function summarizeIssueNumbers(numbers: string[]): string {
  if (numbers.length === 0) return "";
  // Most-recent first → reverse to read low→high before checking
  // contiguity.
  const asInts = numbers
    .map((n) => Number.parseFloat(n))
    .filter((n) => Number.isFinite(n))
    .sort((a, b) => a - b);
  const allContiguous =
    asInts.length === numbers.length &&
    asInts.length >= 2 &&
    asInts.every((n, i) => i === 0 || n === asInts[i - 1]! + 1);
  if (allContiguous) {
    return `#${asInts[0]}–#${asInts[asInts.length - 1]}`;
  }
  const labels = numbers.map((n) => `#${n}`);
  if (labels.length <= 4) return labels.join(", ");
  const head = labels.slice(0, 3).join(", ");
  return `${head} +${labels.length - 3} more`;
}

function PayloadLine({ event }: { event: ReadingLogEventView }) {
  const p: ReadingLogPayload = event.payload;
  switch (p.kind) {
    case "session_completed":
      return (
        <p className="text-muted-foreground truncate text-xs">
          {formatDurationMs(p.active_ms)} · {p.pages_read} page
          {p.pages_read === 1 ? "" : "s"}
          {p.device ? <span> · {p.device}</span> : null}
        </p>
      );
    case "issue_finished":
      return (
        <p className="text-muted-foreground truncate text-xs">
          {creditsLine(event)}
        </p>
      );
    case "series_finished":
      // The server's payload for series_finished currently stubs
      // `total_issues = 0`; surfacing "0 issues read" would be
      // worse than silence, so we only render extra metadata when
      // we have something meaningful to say.
      if (p.total_issues > 0) {
        return (
          <p className="text-muted-foreground truncate text-xs">
            {p.total_issues} issue{p.total_issues === 1 ? "" : "s"} read
            {p.span_days != null && p.span_days > 0 ? (
              <>
                {" "}
                · across {p.span_days} day{p.span_days === 1 ? "" : "s"}
              </>
            ) : null}
          </p>
        );
      }
      return null;
    case "marker_created":
      return (
        <p className="text-muted-foreground truncate text-xs">
          <MarkerKindIcon kind={p.marker_kind} />
          <span className="capitalize">{p.marker_kind}</span>
          <span> · page {p.page_index + 1}</span>
          {p.body_preview ? (
            <span> · &ldquo;{p.body_preview}&rdquo;</span>
          ) : null}
        </p>
      );
  }
}

function MarkerKindIcon({ kind }: { kind: string }) {
  if (kind === "favorite") {
    return (
      <Star
        aria-hidden="true"
        className="text-muted-foreground mr-1 inline h-3 w-3 align-[-2px]"
      />
    );
  }
  if (kind === "note") {
    return (
      <MessageSquare
        aria-hidden="true"
        className="text-muted-foreground mr-1 inline h-3 w-3 align-[-2px]"
      />
    );
  }
  return (
    <BookmarkIcon
      aria-hidden="true"
      className="text-muted-foreground mr-1 inline h-3 w-3 align-[-2px]"
    />
  );
}

function creditsLine(event: ReadingLogEventView): string {
  const i = event.issue;
  if (!i) return "";
  const parts: string[] = [];
  if (i.writer) parts.push(`Writer: ${i.writer}`);
  if (i.penciller && !i.writer) parts.push(`Penciller: ${i.penciller}`);
  if (i.year) parts.push(String(i.year));
  return parts.join(" · ");
}

function FeedSkeleton() {
  return (
    <ol className="flex flex-col gap-4">
      {Array.from({ length: 5 }).map((_, i) => (
        <li key={i} className="flex gap-3.5">
          <Skeleton className="aspect-2/3 w-14 shrink-0 rounded" />
          <div className="flex-1 space-y-2">
            <Skeleton className="h-3 w-1/3" />
            <Skeleton className="h-4 w-2/3" />
            <Skeleton className="h-3 w-1/2" />
          </div>
        </li>
      ))}
    </ol>
  );
}

function EmptyState() {
  return (
    <div className="border-border/60 text-muted-foreground flex flex-col items-center gap-2 rounded-md border border-dashed px-4 py-10 text-center text-sm">
      <p>Nothing in this window yet.</p>
      <p className="text-muted-foreground/80 text-xs">
        Mark an issue read, save a bookmark, or read for a minute — events will
        start landing here.
      </p>
    </div>
  );
}
