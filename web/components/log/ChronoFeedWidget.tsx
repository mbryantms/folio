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

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
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
} from "@/lib/api/types";

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

const KIND_RING: Record<ReadingLogEventKind, string> = {
  issue_finished: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300",
  series_finished: "bg-primary/15 text-primary",
  session_completed: "bg-sky-500/15 text-sky-700 dark:text-sky-300",
  marker_created: "bg-amber-500/15 text-amber-700 dark:text-amber-300",
};

/** Reverse-chronological feed of every reading-activity event the
 *  user has produced. Cursor-paginated via `useReadingLogInfinite`;
 *  an IntersectionObserver sentinel at the tail auto-loads the next
 *  page so the user just keeps scrolling. */
export function ChronoFeedWidget({ filters }: { filters: ReadingLogFilters }) {
  const query = useReadingLogInfinite(filters);
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    const node = sentinelRef.current;
    if (!node) return;
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
      { rootMargin: "240px" },
    );
    obs.observe(node);
    return () => obs.disconnect();
  }, [query]);

  const events: ReadingLogEventView[] = React.useMemo(
    () => query.data?.pages.flatMap((p) => p.events) ?? [],
    [query.data],
  );

  // Group consecutive events from the same series on the same day
  // (user-local). Reduces visual noise when a user clears a 12-issue
  // arc in one sitting — every issue gets its own row, but they all
  // sit under a single "Saga of the Swamp Thing · May 19" header.
  const groups = React.useMemo(() => groupEvents(events), [events]);

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="text-base">Activity</CardTitle>
      </CardHeader>
      <CardContent>
        {query.isLoading ? (
          <FeedSkeleton />
        ) : events.length === 0 ? (
          <EmptyState />
        ) : (
          <ol className="flex flex-col gap-5">
            {groups.map((g) => (
              <li key={g.key} className="flex flex-col gap-2">
                <GroupHeader group={g} />
                <ul className="border-border/60 flex flex-col gap-3 border-l-2 pl-4">
                  {g.events.map((e) => (
                    <li key={e.id}>
                      <EventRow event={e} />
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
      </CardContent>
    </Card>
  );
}

type Group = {
  key: string;
  seriesId: string | null;
  seriesName: string | null;
  seriesSlug: string | null;
  dayLabel: string;
  events: ReadingLogEventView[];
};

function dayKey(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, {
    weekday: "long",
    month: "short",
    day: "numeric",
  });
}

function groupEvents(events: ReadingLogEventView[]): Group[] {
  const groups: Group[] = [];
  for (const e of events) {
    const day = dayKey(e.occurred_at);
    const sid = e.series?.id ?? null;
    const last = groups[groups.length - 1];
    if (last && last.dayLabel === day && last.seriesId === sid) {
      last.events.push(e);
    } else {
      groups.push({
        key: `${day}|${sid ?? "none"}|${e.id}`,
        seriesId: sid,
        seriesName: e.series?.name ?? null,
        seriesSlug: e.series?.slug ?? null,
        dayLabel: day,
        events: [e],
      });
    }
  }
  return groups;
}

function GroupHeader({ group }: { group: Group }) {
  return (
    <div className="text-muted-foreground flex flex-wrap items-baseline gap-x-2 text-xs">
      {group.seriesSlug && group.seriesName ? (
        <Link
          href={seriesUrl(group.seriesSlug)}
          className="hover:text-foreground text-foreground/80 truncate text-sm font-medium"
          title={group.seriesName}
        >
          {group.seriesName}
        </Link>
      ) : group.seriesName ? (
        <span className="text-foreground/80 truncate text-sm font-medium">
          {group.seriesName}
        </span>
      ) : null}
      <span>·</span>
      <time>{group.dayLabel}</time>
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
    <div className="hover:bg-muted/50 group/event flex gap-3 rounded-md p-1.5 transition-colors">
      <div
        className={cn(
          "border-border/60 relative h-14 w-10 shrink-0 overflow-hidden rounded border",
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
        <div className="flex flex-wrap items-center gap-1.5">
          <span
            className={cn(
              "inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[10px] font-medium tracking-wide uppercase",
              KIND_RING[event.kind],
            )}
          >
            <Icon aria-hidden="true" className="h-3 w-3" />
            {KIND_LABEL[event.kind]}
          </span>
          {issueLabel ? (
            <span className="text-muted-foreground text-xs font-medium tabular-nums">
              {issueLabel}
            </span>
          ) : null}
          <time
            className="text-muted-foreground/80 ml-auto text-xs"
            title={new Date(event.occurred_at).toLocaleString()}
          >
            {formatRelativeDate(event.occurred_at)}
          </time>
        </div>
        <div className="truncate text-sm font-medium" title={headline}>
          {headline}
        </div>
        <PayloadLine event={event} />
      </div>
    </div>
  );

  // Most kinds deep-link to the issue; series_finished deep-links to
  // the series detail page. Marker events would ideally route to the
  // marker's anchor page in the reader, but the marker page-index +
  // reader URL plumbing is a separate concern — M2 routes to the
  // issue and lets the user open the bookmarks tab there.
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
      return (
        <p className="text-muted-foreground truncate text-xs">
          {p.total_issues > 0 ? (
            <>
              {p.total_issues} issue{p.total_issues === 1 ? "" : "s"} read
            </>
          ) : (
            <>Series complete</>
          )}
          {p.span_days != null && p.span_days > 0 ? (
            <>
              {" "}
              · across {p.span_days} day{p.span_days === 1 ? "" : "s"}
            </>
          ) : null}
        </p>
      );
    case "marker_created":
      return (
        <p className="text-muted-foreground truncate text-xs">
          <MarkerKindIcon kind={p.marker_kind} />
          <span className="capitalize">{p.marker_kind}</span>
          <span> · page {p.page_index + 1}</span>
          {p.body_preview ? <span> · “{p.body_preview}”</span> : null}
        </p>
      );
  }
}

function MarkerKindIcon({ kind }: { kind: string }) {
  if (kind === "favorite")
    return (
      <Star
        aria-hidden="true"
        className="text-muted-foreground mr-1 inline h-3 w-3 align-[-2px]"
      />
    );
  if (kind === "note")
    return (
      <MessageSquare
        aria-hidden="true"
        className="text-muted-foreground mr-1 inline h-3 w-3 align-[-2px]"
      />
    );
  return (
    <BookmarkIcon
      aria-hidden="true"
      className="text-muted-foreground mr-1 inline h-3 w-3 align-[-2px]"
    />
  );
}

/** Best-effort credit string for the row's secondary line. We surface
 *  the writer when present; pencillers fold in only if there's no
 *  writer, since most rows that have either have both. */
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
        <li key={i} className="flex gap-3">
          <Skeleton className="h-14 w-10 shrink-0 rounded" />
          <div className="flex-1 space-y-2">
            <Skeleton className="h-3 w-1/3" />
            <Skeleton className="h-3 w-2/3" />
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
