"use client";

import Link from "next/link";
import { useMemo } from "react";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  formatDayLabel,
  formatDurationMs,
  formatTimeOfDay,
  groupSessionsByDay,
  labelForSession,
} from "@/lib/activity";
import { useReadingSessions, type ReadingStatsScope } from "@/lib/api/queries";
import type { ReadingSessionView } from "@/lib/api/types";

/**
 * Scope-aware reading-session timeline. Used by `/settings/activity` (scope
 * = all), the series page Activity tab (scope = series, id), and the issue
 * page Activity tab (scope = issue, id). Always shows the *current user's*
 * sessions — server enforces.
 */
export function ActivityTimeline({
  scope,
  emptyHint,
}: {
  scope: ReadingStatsScope;
  /** Required for issue-row links. Pass from the page that hosts the timeline. */
  /** Optional override for the no-data state — series and issue pages
   *  use this to point at the relevant Read CTA instead of the generic
   *  "open an issue" copy. */
  emptyHint?: React.ReactNode;
}) {
  const filters = useMemo(() => {
    if (scope.type === "series") return { series_id: scope.id, limit: 50 };
    if (scope.type === "issue") return { issue_id: scope.id, limit: 50 };
    return { limit: 50 };
  }, [scope]);

  const sessions = useReadingSessions(filters);

  if (sessions.isLoading) return <Skeleton className="h-32 w-full" />;
  if (sessions.error) {
    return <p className="text-destructive text-sm">Failed to load sessions.</p>;
  }
  const records = (sessions.data?.pages ?? []).flatMap((p) => p.records);

  if (records.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        {emptyHint ??
          "No reading sessions yet — open an issue and read for at least 30 seconds."}
      </p>
    );
  }

  const grouped = groupSessionsByDay(records);
  return (
    <div className="space-y-5">
      {[...grouped.entries()].map(([day, rows]) => (
        <div key={day}>
          <h3 className="text-muted-foreground mb-1.5 text-xs font-semibold tracking-wide uppercase">
            {formatDayLabel(day)}
          </h3>
          <ul className="divide-border divide-y">
            {rows.map((s) => (
              <SessionRow key={s.id} session={s} scope={scope} />
            ))}
          </ul>
        </div>
      ))}
      {sessions.hasNextPage ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => sessions.fetchNextPage()}
          disabled={sessions.isFetchingNextPage}
        >
          {sessions.isFetchingNextPage ? "Loading…" : "Load more"}
        </Button>
      ) : null}
    </div>
  );
}

function SessionRow({
  session,
  scope,
}: {
  session: ReadingSessionView;
  scope: ReadingStatsScope;
}) {
  const label = labelForSession(session);
  const subtitle = (
    <>
      {formatTimeOfDay(session.started_at)} · pages {session.start_page + 1}–
      {session.end_page + 1}
      {session.view_mode ? ` · ${session.view_mode}` : ""}
    </>
  );
  const href = `/issues/${session.issue_id}`;
  // On the issue-scoped Activity tab the issue identity is implied; render
  // a non-link row to keep the page focused on the current issue.
  const showLink = scope.type !== "issue";

  return (
    <li className="flex items-center justify-between gap-3 py-2 text-sm">
      <div className="min-w-0">
        {showLink ? (
          <Link
            href={href}
            className="text-foreground block truncate font-medium hover:underline"
          >
            {label}
          </Link>
        ) : (
          <p className="text-foreground truncate font-medium">{label}</p>
        )}
        <p className="text-muted-foreground text-xs">{subtitle}</p>
      </div>
      <div className="flex shrink-0 items-baseline gap-2">
        <span className="text-foreground text-sm font-medium">
          {formatDurationMs(session.active_ms)}
        </span>
        <span className="text-muted-foreground text-xs">
          {session.distinct_pages_read} pp
        </span>
      </div>
    </li>
  );
}
