"use client";

import Link from "next/link";

import { Skeleton } from "@/components/ui/skeleton";
import { useContinueReading } from "@/lib/api/queries";
import { issueUrl } from "@/lib/urls";

import { WidgetCard } from "../WidgetCard";
import type { CurrentlyReadingConfig, LogWidgetProps } from "./types";

/** In-progress reads — same data the home page's Continue Reading
 *  rail uses, but laid out as a vertical list for the log's grid.
 *  Caps to `config.limit` rows. */
export function CurrentlyReading({
  widget,
}: LogWidgetProps<CurrentlyReadingConfig>) {
  const limit = widget.config.limit ?? 5;
  const query = useContinueReading();
  const items = (query.data?.items ?? []).slice(0, limit);

  return (
    <WidgetCard widgetId={widget.id} title="Currently reading">
      {query.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-12 w-full" />
          <Skeleton className="h-12 w-full" />
        </div>
      ) : items.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          Nothing in progress right now — start an issue and it&rsquo;ll land
          here.
        </p>
      ) : (
        <ol className="flex flex-col gap-2">
          {items.map((card) => {
            const i = card.issue;
            const pct = Math.max(0, Math.min(100, card.progress.percent * 100));
            return (
              <li key={i.id}>
                <Link
                  href={issueUrl(i.series_slug, i.slug)}
                  className="hover:bg-muted/50 flex items-center gap-2.5 rounded-md p-1.5 transition-colors"
                >
                  <div className="border-border/60 bg-muted relative h-14 w-10 shrink-0 overflow-hidden rounded border">
                    {i.cover_url ? (
                      // eslint-disable-next-line @next/next/no-img-element
                      <img
                        src={i.cover_url}
                        alt=""
                        className="h-full w-full object-cover"
                        loading="lazy"
                      />
                    ) : null}
                  </div>
                  <div className="flex min-w-0 flex-1 flex-col gap-1">
                    <span
                      className="truncate text-sm font-medium"
                      title={card.series_name}
                    >
                      {card.series_name}
                      {i.number ? (
                        <span className="text-muted-foreground/80 ml-1 font-normal tabular-nums">
                          #{i.number}
                        </span>
                      ) : null}
                    </span>
                    <div
                      aria-label={`${Math.round(pct)}% read`}
                      className="bg-muted/40 h-1 overflow-hidden rounded-full"
                    >
                      <div
                        className="bg-primary h-full rounded-full"
                        style={{ width: `${pct}%` }}
                      />
                    </div>
                  </div>
                </Link>
              </li>
            );
          })}
        </ol>
      )}
    </WidgetCard>
  );
}
