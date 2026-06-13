"use client";

import * as React from "react";
import Link from "next/link";
import {
  BookmarkIcon,
  Check,
  ChevronDown,
  EyeOff,
  ListChecks,
  MessageSquare,
  MoreVertical,
  Star,
  Timer,
} from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import {
  eventIdToSourceId,
  useHideReadingLogEvent,
  useUnhideReadingLogEvent,
} from "@/lib/api/mutations";
import { useReadingLogInfinite } from "@/lib/api/queries";
import { formatDurationMs } from "@/lib/activity";
import { formatRelativeDate } from "@/lib/format";
import { issueUrl, seriesUrl } from "@/lib/urls";
import { statusTone } from "@/lib/ui/status-tone";
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

/** Reverse-chronological feed of every reading-activity event the
 *  user has produced. Cursor-paginated; an IntersectionObserver
 *  sentinel at the tail auto-loads the next page on scroll inside
 *  the bounded-height container.
 *
 *  Grouping is configurable (`day` / `week` / `month` / `none`).
 *  Every event renders as its own row — same-series runs no longer
 *  collapse into a summary line because the rolled-up rows hid the
 *  per-issue cadence we want to surface here. */
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
  // "Show hidden" lives in client-only state — it's a viewing
  // preference, not a config value worth persisting per-widget /
  // per-user. Toggling it re-fires the underlying infinite query
  // because `include_hidden` is part of the filter cache key.
  const [showHidden, setShowHidden] = React.useState(false);
  const filters: ReadingLogFilters = React.useMemo(() => {
    const widgetKinds = widget.config.default_kinds ?? [];
    const kinds = widgetKinds.length > 0 ? widgetKinds : undefined;
    return {
      kinds,
      from: rangeToFrom(effectiveRange),
      limit: 30,
      include_hidden: showHidden,
    };
  }, [effectiveRange, showHidden, widget.config.default_kinds]);

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
    <WidgetCard
      widget={widget}
      title="Activity"
      titleHref="/log/activity"
      extraMenuItems={
        // Lives in the widget's kebab so the feed body stays
        // chrome-free — most users never touch this toggle, and an
        // always-visible row of controls above the feed felt heavier
        // than the affordance warranted. Checkbox state is the same
        // `showHidden` that drives the underlying `include_hidden`
        // filter; flipping it repolls the feed.
        <DropdownMenuCheckboxItem
          checked={showHidden}
          onCheckedChange={(v) => setShowHidden(v === true)}
          onSelect={(e) => e.preventDefault()}
        >
          Show hidden
        </DropdownMenuCheckboxItem>
      }
    >
      <div ref={scrollRef} className="max-h-160 overflow-y-auto pr-1">
        {query.isLoading ? (
          <FeedSkeleton />
        ) : events.length === 0 ? (
          <EmptyState />
        ) : (
          <ol className="flex flex-col gap-4">
            {groups.map((g) => (
              <li key={g.key} className="flex flex-col gap-2">
                {g.label ? <GroupHeader label={g.label} /> : null}
                {/* Tighter row gap and no leading timeline border —
                 *  the day header already groups visually, and the
                 *  per-row hover background is the affordance the
                 *  user reads as "selectable". */}
                <ul className="flex flex-col gap-1">
                  {g.rows.map((event) => (
                    <li key={event.id}>
                      <EventRow event={event} />
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

// ─── Grouping ───

type Section = {
  key: string;
  /** `null` for the flat (no-group) layout. */
  label: string | null;
  rows: ReadingLogEventView[];
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
    section.rows.push(e);
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
  const Icon = KIND_ICON[event.kind as ReadingLogEventKind];
  const cover = event.issue?.cover_url ?? event.series?.cover_url ?? null;
  const issueLabel = event.issue?.number ? `#${event.issue.number}` : null;
  const issueTitle = event.issue?.title ?? null;
  const seriesName = event.series?.name ?? null;
  // Prefer the series name as the headline whenever we have one — the
  // feed scans much more naturally as a list of series + issue numbers
  // than as a list of (often empty) issue titles. Issue title moves
  // into the subtitle slot when present.
  const headline = seriesName ?? issueTitle ?? "Reading event";
  const subtitle =
    seriesName && issueTitle && issueTitle !== seriesName ? issueTitle : null;
  const isHidden = event.is_hidden === true;

  const inner = (
    // gap-3 (was gap-4) between thumb and content; p-1.5 (was p-2)
    // around the card. Tightens vertical rhythm so more rows fit
    // above the fold without the chrome competing with the content.
    // `opacity-60` for hidden rows (only surfaced when the "Show
    // hidden" toggle is on) so the user can scan them at a glance and
    // distinguish from active ones.
    <div
      className={cn(
        "hover:bg-muted/50 group/event flex gap-3 rounded-md p-1.5 transition-colors",
        isHidden && "opacity-60",
      )}
    >
      <div
        className={cn(
          "border-border/60 relative aspect-2/3 w-16 shrink-0 self-start overflow-hidden rounded border",
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
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {/* Top row: headline + issue number on the left (flex-1
         *  truncating), timestamp pinned to the right with a baseline
         *  alignment so it sits on the headline's text baseline
         *  instead of free-floating at the top edge. */}
        <div className="flex items-baseline gap-2">
          <div className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2">
            <span className="truncate text-sm font-semibold" title={headline}>
              {headline}
            </span>
            {issueLabel ? (
              <span className="text-muted-foreground text-xs font-medium tabular-nums">
                {issueLabel}
              </span>
            ) : null}
          </div>
          <time
            className="text-muted-foreground/60 shrink-0 text-[11px]"
            title={new Date(event.occurred_at).toLocaleString()}
          >
            {formatRelativeDate(event.occurred_at)}
          </time>
        </div>
        {subtitle ? (
          <p
            className="text-muted-foreground/90 truncate text-xs"
            title={subtitle}
          >
            {subtitle}
          </p>
        ) : null}
        {/* Bottom row: kind chip + payload line. `min-w-0` on the
         *  PayloadLine wrapper lets it truncate instead of pushing
         *  the chip off-screen on narrow widths. */}
        <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 pt-0.5">
          <span
            className={cn(
              "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium tracking-wide uppercase",
              KIND_TINT[event.kind as ReadingLogEventKind],
            )}
          >
            <Icon aria-hidden="true" className="h-3 w-3" />
            {KIND_LABEL[event.kind as ReadingLogEventKind]}
          </span>
          <PayloadLine event={event} />
        </div>
      </div>
    </div>
  );

  // Navigation target — derived event has no issue, so it routes to
  // the series; everything else routes to the issue detail.
  const navHref =
    event.kind === "series_finished" && event.series
      ? seriesUrl(event.series.slug)
      : event.issue && event.series
        ? issueUrl(event.series.slug, event.issue.slug)
        : null;

  // Kebab menu adjacent to the link (not nested — `<button>` inside
  // `<a>` is invalid HTML). `series_finished` doesn't expose hide
  // because the event is derived from MAX(finished_at); there's no
  // single row to flag.
  const menu =
    event.kind === "series_finished" ? null : <RowKebab event={event} />;

  return (
    <div className="group/event-row relative">
      {navHref ? <Link href={navHref}>{inner}</Link> : inner}
      {menu ? <div className="absolute top-1.5 right-1.5">{menu}</div> : null}
    </div>
  );
}

/** Per-row kebab menu — currently exposes Hide / Show again for the
 *  three hideable event kinds. Sits absolutely at the top-right of
 *  the row, fades in on hover (or always when focused). */
function RowKebab({ event }: { event: ReadingLogEventView }) {
  const hide = useHideReadingLogEvent();
  const unhide = useUnhideReadingLogEvent();
  const isHidden = event.is_hidden === true;
  const kind = event.kind as
    | "issue_finished"
    | "session_completed"
    | "marker_created";
  const sourceId = eventIdToSourceId(event.id);
  if (!sourceId) return null;
  const pending = hide.isPending || unhide.isPending;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label="Activity event actions"
        disabled={pending}
        className={cn(
          "bg-background/80 text-muted-foreground hover:text-foreground inline-flex h-7 w-7 items-center justify-center rounded-md backdrop-blur transition-opacity focus-visible:opacity-100 focus-visible:outline-none",
          // Always visible on touch; fade in on hover (desktop) so
          // resting state stays clean.
          "opacity-0 group-hover/event-row:opacity-100",
        )}
      >
        <MoreVertical aria-hidden="true" className="h-4 w-4" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-44">
        {isHidden ? (
          <DropdownMenuItem
            onSelect={() => unhide.mutate({ kind, source_id: sourceId })}
            disabled={pending}
          >
            <EyeOff className="mr-2 h-4 w-4" aria-hidden="true" />
            Show again
          </DropdownMenuItem>
        ) : (
          <DropdownMenuItem
            onSelect={() => hide.mutate({ kind, source_id: sourceId })}
            disabled={pending}
          >
            <EyeOff className="mr-2 h-4 w-4" aria-hidden="true" />
            Hide from activity
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function PayloadLine({ event }: { event: ReadingLogEventView }) {
  const p: ReadingLogPayload = event.payload;
  switch (p.kind) {
    case "session_completed":
      return (
        <span className="text-muted-foreground min-w-0 truncate text-xs">
          {formatDurationMs(p.active_ms)} · {p.pages_read} page
          {p.pages_read === 1 ? "" : "s"}
          {p.device ? <span> · {p.device}</span> : null}
        </span>
      );
    case "issue_finished": {
      const credits = creditsLine(event);
      if (!credits) return null;
      return (
        <span className="text-muted-foreground min-w-0 truncate text-xs">
          {credits}
        </span>
      );
    }
    case "series_finished":
      // The server's payload for series_finished currently stubs
      // `total_issues = 0`; surfacing "0 issues read" would be
      // worse than silence, so we only render extra metadata when
      // we have something meaningful to say.
      if (p.total_issues > 0) {
        return (
          <span className="text-muted-foreground min-w-0 truncate text-xs">
            {p.total_issues} issue{p.total_issues === 1 ? "" : "s"} read
            {p.span_days != null && p.span_days > 0 ? (
              <>
                {" "}
                · across {p.span_days} day{p.span_days === 1 ? "" : "s"}
              </>
            ) : null}
          </span>
        );
      }
      return null;
    case "marker_created":
      return (
        <span className="text-muted-foreground min-w-0 truncate text-xs">
          <MarkerKindIcon kind={p.marker_kind} />
          <span className="capitalize">{p.marker_kind}</span>
          <span> · page {p.page_index + 1}</span>
          {p.body_preview ? (
            <span> · &ldquo;{p.body_preview}&rdquo;</span>
          ) : null}
        </span>
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
    <ol className="flex flex-col gap-3">
      {Array.from({ length: 5 }).map((_, i) => (
        <li key={i} className="flex gap-4">
          <Skeleton className="aspect-2/3 w-20 shrink-0 rounded" />
          <div className="flex-1 space-y-2 pt-1">
            <Skeleton className="h-4 w-2/3" />
            <Skeleton className="h-3 w-1/3" />
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
