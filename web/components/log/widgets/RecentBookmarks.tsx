"use client";

import * as React from "react";
import Link from "next/link";
import { BookmarkIcon, MessageSquare, Star } from "lucide-react";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingLogInfinite } from "@/lib/api/queries";
import { formatRelativeDate } from "@/lib/format";
import { issueUrl } from "@/lib/urls";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps, RecentBookmarksConfig } from "./types";

function KindIcon({ kind }: { kind: string }) {
  const className = "text-muted-foreground h-3.5 w-3.5 shrink-0";
  if (kind === "favorite")
    return <Star aria-hidden="true" className={className} />;
  if (kind === "note")
    return <MessageSquare aria-hidden="true" className={className} />;
  return <BookmarkIcon aria-hidden="true" className={className} />;
}

/** Recent marker-created events in reverse-chronological order. Pulls
 *  from the reading-log feed scoped to `kind=marker_created`. The
 *  page-level kind chips do **not** narrow this widget — it always
 *  shows markers — so users with the page filtered to (say) "just
 *  sessions" still see new bookmarks land here. */
export function RecentBookmarks({
  widget,
}: LogWidgetProps<RecentBookmarksConfig>) {
  const limit = widget.config.limit ?? 5;
  const allowedKinds = widget.config.kinds ?? [];
  const query = useReadingLogInfinite(
    React.useMemo(() => ({ kinds: ["marker_created"], limit }), [limit]),
  );
  const events = (query.data?.pages.flatMap((p) => p.events) ?? [])
    .filter((e) => {
      if (allowedKinds.length === 0) return true;
      if (e.payload.kind !== "marker_created") return false;
      return allowedKinds.includes(e.payload.marker_kind);
    })
    .slice(0, limit);

  return (
    <WidgetCard widgetId={widget.id} title="Recent bookmarks">
      {query.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-3 w-3/4" />
          <Skeleton className="h-3 w-2/3" />
          <Skeleton className="h-3 w-1/2" />
        </div>
      ) : events.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          No bookmarks yet — save one from the reader to populate.
        </p>
      ) : (
        <ol className="flex flex-col gap-2">
          {events.map((e) => {
            if (e.payload.kind !== "marker_created") return null;
            const series = e.series;
            const issue = e.issue;
            const inner = (
              <div className="hover:bg-muted/50 flex items-start gap-2 rounded-md p-1 transition-colors">
                <KindIcon kind={e.payload.marker_kind} />
                <div className="flex min-w-0 flex-1 flex-col">
                  <span
                    className="truncate text-sm font-medium"
                    title={series?.name ?? "Bookmark"}
                  >
                    {series?.name ?? "Bookmark"}
                    {issue?.number ? (
                      <span className="text-muted-foreground/80 ml-1 font-normal tabular-nums">
                        #{issue.number}
                      </span>
                    ) : null}
                  </span>
                  {e.payload.body_preview ? (
                    <span className="text-muted-foreground line-clamp-2 text-xs">
                      “{e.payload.body_preview}”
                    </span>
                  ) : (
                    <span className="text-muted-foreground/80 text-xs">
                      page {e.payload.page_index + 1}
                    </span>
                  )}
                  <span className="text-muted-foreground/70 text-[10px]">
                    {formatRelativeDate(e.occurred_at)}
                  </span>
                </div>
              </div>
            );
            return (
              <li key={e.id}>
                {series && issue ? (
                  <Link href={issueUrl(series.slug, issue.slug)}>{inner}</Link>
                ) : (
                  inner
                )}
              </li>
            );
          })}
        </ol>
      )}
    </WidgetCard>
  );
}
