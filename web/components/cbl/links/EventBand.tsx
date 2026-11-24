"use client";

/**
 * Linked reading lists (PR 2b): an event read at this point of a master
 * list, drawn as a full-width band across the cover grid rather than one
 * more cell, so it reads as a detour off the main line.
 *
 * States: not started / in progress → full band; finished or skipped →
 * one line (skipped with Undo); a link whose anchor issue a refresh
 * removed → full band with a "placement lost" badge.
 */
import * as React from "react";
import Link from "next/link";
import {
  ChevronDown,
  CircleCheck,
  CornerDownRight,
  MoreHorizontal,
  TriangleAlert,
  Zap,
} from "lucide-react";

import { CoverStack } from "@/components/cover-stack/CoverStack";
import { Cover } from "@/components/Cover";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Progress } from "@/components/ui/progress";
import { useCblListEntriesInfinite } from "@/lib/api/queries";
import type { ReadingListLinkView } from "@/lib/api/types";
import { readerUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

/** How many of the event's covers the band loads (stack + peek row). */
export const EVENT_PEEK_LIMIT = 12;

export type EventBandState = "full" | "finished" | "skipped";

/** Which way a band draws for a link. Pure, for tests. */
export function eventBandState(link: ReadingListLinkView): EventBandState {
  if (link.skipped) return "skipped";
  if (link.in_library > 0 && link.read >= link.in_library) return "finished";
  return "full";
}

/** "6 / 64 read · resume at …" — the band's progress line. */
export function eventProgressLabel(link: ReadingListLinkView): string {
  const base = `${link.read} / ${link.total} read`;
  if (link.read === 0) return base;
  return link.first_unread ? `${base} · in progress` : base;
}

export function EventBand({
  link,
  afterLabel,
  canManage,
  onSkip,
  onMove,
  onRemove,
}: {
  link: ReadingListLinkView;
  /** "Excalibur #12": the issue the event comes after. */
  afterLabel: string | null;
  canManage: boolean;
  onSkip: (skip: boolean) => void;
  onMove?: () => void;
  onRemove?: () => void;
}) {
  const state = eventBandState(link);
  const viewHref = link.child_view_id ? `/views/${link.child_view_id}` : null;
  const startHref = link.first_unread
    ? readerUrl(link.first_unread.series_slug, link.first_unread.issue_slug, {
        cbl: link.child_view_id,
      })
    : null;
  const percent = link.total > 0 ? (link.read / link.total) * 100 : 0;
  const lost = link.anchor?.status === "lost";

  const menu = (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={`Actions for ${link.child_name}`}
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {viewHref && (
          <DropdownMenuItem asChild>
            <Link href={viewHref}>Open list</Link>
          </DropdownMenuItem>
        )}
        <DropdownMenuItem onSelect={() => onSkip(!link.skipped)}>
          {link.skipped ? "Stop skipping" : "Skip this event"}
        </DropdownMenuItem>
        {canManage && onMove && (
          <DropdownMenuItem onSelect={onMove}>Move…</DropdownMenuItem>
        )}
        {canManage && onRemove && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={onRemove}>
              Remove from this list
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );

  if (state !== "full") {
    return (
      <div
        data-testid="event-band"
        data-state={state}
        className="border-border/60 flex min-h-11 items-center gap-2 rounded-md border px-3 py-1.5"
      >
        {state === "finished" ? (
          <CircleCheck
            className="size-4 shrink-0 text-emerald-500"
            aria-hidden
          />
        ) : (
          <CornerDownRight
            className="text-muted-foreground size-4 shrink-0"
            aria-hidden
          />
        )}
        <span className="truncate text-sm font-medium">{link.child_name}</span>
        <span className="text-muted-foreground shrink-0 text-xs">
          {state === "finished"
            ? `${link.read} / ${link.total} read`
            : "Skipped"}
        </span>
        <span className="ml-auto flex shrink-0 items-center gap-1">
          {state === "skipped" && (
            <Button variant="ghost" size="sm" onClick={() => onSkip(false)}>
              Undo
            </Button>
          )}
          {menu}
        </span>
      </div>
    );
  }

  return (
    <EventBandFull
      link={link}
      afterLabel={afterLabel}
      lost={lost}
      percent={percent}
      startHref={startHref}
      viewHref={viewHref}
      menu={menu}
    />
  );
}

function EventBandFull({
  link,
  afterLabel,
  lost,
  percent,
  startHref,
  viewHref,
  menu,
}: {
  link: ReadingListLinkView;
  afterLabel: string | null;
  lost: boolean;
  percent: number;
  startHref: string | null;
  viewHref: string | null;
  menu: React.ReactNode;
}) {
  const [peekOpen, setPeekOpen] = React.useState(false);
  const covers = useCblListEntriesInfinite(link.child_list_id, {
    limit: EVENT_PEEK_LIMIT,
  });
  const items = covers.data?.pages[0]?.items ?? [];
  const coverUrls = items
    .map((i) => i.issue?.cover_url)
    .filter((u): u is string => !!u);
  const started = link.read > 0;

  return (
    <Collapsible open={peekOpen} onOpenChange={setPeekOpen}>
      <section
        data-testid="event-band"
        data-state="full"
        aria-label={`Event: ${link.child_name}`}
        className="cover-stack-host border-border bg-card rounded-lg border p-3 sm:p-4"
      >
        <div className="text-muted-foreground mb-2 flex items-center gap-2 text-xs">
          <Zap className="size-3.5 text-amber-500" aria-hidden />
          <span className="truncate">
            Event{afterLabel ? ` · read after ${afterLabel}` : ""}
          </span>
          {lost && (
            <Badge variant="outline" className="gap-1">
              <TriangleAlert className="size-3" aria-hidden />
              Placement lost
            </Badge>
          )}
          <Badge variant="secondary" className="ml-auto shrink-0">
            {link.in_library} / {link.total} in library
          </Badge>
        </div>
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
          <div className="w-16 shrink-0 sm:w-20">
            <CoverStack urls={coverUrls} spill={false} />
          </div>
          <div className="min-w-0 flex-1">
            <h3 className="truncate text-base font-semibold">
              {viewHref ? (
                <Link href={viewHref} className="hover:underline">
                  {link.child_name}
                </Link>
              ) : (
                link.child_name
              )}
            </h3>
            <p className="text-muted-foreground mb-2 text-xs">
              {link.total} {link.total === 1 ? "issue" : "issues"}
            </p>
            <Progress
              value={percent}
              aria-label={`${link.child_name} progress`}
              className="h-1.5"
            />
            <p className="text-muted-foreground mt-1 text-xs">
              {eventProgressLabel(link)}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {startHref && (
              <Button asChild size="sm">
                <Link href={startHref}>{started ? "Resume" : "Start"}</Link>
              </Button>
            )}
            {items.length > 0 && (
              <CollapsibleTrigger asChild>
                <Button variant="outline" size="sm" aria-expanded={peekOpen}>
                  <ChevronDown
                    className={cn(
                      "size-4 transition-transform",
                      peekOpen && "rotate-180",
                    )}
                    aria-hidden
                  />
                  Peek
                </Button>
              </CollapsibleTrigger>
            )}
            {menu}
          </div>
        </div>
        <CollapsibleContent>
          <ul
            role="list"
            aria-label={`First issues of ${link.child_name}`}
            className="mt-3 flex gap-2 overflow-x-auto pb-1"
          >
            {items.map((item) => (
              <li key={item.id} className="w-14 shrink-0 sm:w-16">
                {item.issue ? (
                  <Link
                    href={readerUrl(item.issue, { cbl: link.child_view_id })}
                    aria-label={`Read ${item.series_name} #${item.issue_number}`}
                  >
                    <Cover
                      src={item.issue.cover_url}
                      tone={item.issue.cover_tone}
                      alt=""
                      className="aspect-[2/3] w-full rounded-sm"
                    />
                  </Link>
                ) : (
                  <div
                    className="border-border/60 aspect-[2/3] w-full rounded-sm border border-dashed"
                    title={`${item.series_name} #${item.issue_number} — not in your library`}
                  />
                )}
              </li>
            ))}
          </ul>
        </CollapsibleContent>
      </section>
    </Collapsible>
  );
}
