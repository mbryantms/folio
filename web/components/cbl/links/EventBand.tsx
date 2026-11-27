"use client";

/**
 * Linked reading lists (PR 2b): an event read at this point of a master
 * list, drawn as a full-width band across the cover grid rather than one
 * more cell, so it reads as a detour off the main line.
 *
 * The full band is a hero: the event's covers fanned large, the lead
 * cover's colours washed behind it, and a strip of its first issues at
 * a size a cover can be read at.
 *
 * States: not started / in progress → full band; finished or skipped →
 * one line (skipped with Unskip); a link whose anchor issue a refresh
 * removed → full band with a "placement lost" badge.
 *
 * Opening the list (title, "Open list", Start) goes through the reader's
 * saved view of it. A reader who has none (a shared event they never
 * saved) gets one on the first click — `POST /me/cbl-lists/{id}/view` —
 * so the band always opens its list and reads in its order.
 */
import * as React from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import {
  CircleCheck,
  CornerDownRight,
  MoreHorizontal,
  Play,
  TriangleAlert,
  Zap,
} from "lucide-react";

import { CoverStack } from "@/components/cover-stack/CoverStack";
import { Cover } from "@/components/Cover";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Progress } from "@/components/ui/progress";
import { useOpenCblListView } from "@/lib/api/mutations";
import { useCblListEntriesInfinite } from "@/lib/api/queries";
import type { ReadingListLinkView } from "@/lib/api/types";
import { progressLabel } from "@/lib/cbl/linked-plan";
import { displayListName } from "@/lib/reader/list-context";
import { readerUrl } from "@/lib/urls";
import { decodeTone } from "@/lib/ui/cover-tone-style";
import { statusToneText } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

/** How many of the event's covers the band loads (stack + cover strip). */
export const EVENT_COVER_LIMIT = 12;

/** "all 19 in library" / "12 of 19 in library" — how much of the event
 *  the reader owns. Pure, for tests. */
export function eventLibraryLabel(link: ReadingListLinkView): string {
  if (link.total === 0) return "no issues";
  const issues = `${link.total} ${link.total === 1 ? "issue" : "issues"}`;
  if (link.in_library >= link.total) return `${issues} · all in library`;
  if (link.in_library === 0) return `${issues} · none in library`;
  return `${issues} · ${link.in_library} in library`;
}

export type EventBandState = "full" | "finished" | "skipped";

/** Which way a band draws for a link. Pure, for tests. */
export function eventBandState(link: ReadingListLinkView): EventBandState {
  if (link.skipped) return "skipped";
  if (link.in_library > 0 && link.read >= link.in_library) return "finished";
  return "full";
}

/** "6 / 64 read · in progress" — the band's progress line. */
export function eventProgressLabel(link: ReadingListLinkView): string {
  const base = progressLabel(link);
  if (link.read === 0) return base;
  return link.first_unread ? `${base} · in progress` : base;
}

/** Navigate through the reader's view of the event list, creating it
 *  first when they have none. */
function useOpenThroughView(parentListId: string, link: ReadingListLinkView) {
  const router = useRouter();
  const open = useOpenCblListView(parentListId);
  return {
    pending: open.isPending,
    go: (to: (viewId: string) => string) => {
      if (link.child_view_id) {
        router.push(to(link.child_view_id));
        return;
      }
      open.mutate(
        { listId: link.child_list_id },
        { onSuccess: (d) => d && router.push(to(d.view_id)) },
      );
    },
  };
}

export function EventBand({
  link,
  parentListId,
  parentViewId = null,
  afterLabel,
  canManage,
  onSkip,
  onMove,
  onMoveEarlier,
  onMoveLater,
  onRemove,
}: {
  link: ReadingListLinkView;
  /** The master list the band sits in. */
  parentListId: string;
  /** The reader's view of that master: carried as `?via=` into the event
   *  so its page links back, and its last issue returns, here. */
  parentViewId?: string | null;
  /** "Excalibur #12": the issue the event comes after. */
  afterLabel: string | null;
  canManage: boolean;
  onSkip: (skip: boolean) => void;
  onMove?: () => void;
  /** Among several events read after the same issue; absent at an end. */
  onMoveEarlier?: () => void;
  onMoveLater?: () => void;
  onRemove?: () => void;
}) {
  const state = eventBandState(link);
  const opener = useOpenThroughView(parentListId, link);
  const via = parentViewId;
  const viewPath = (id: string) =>
    via ? `/views/${id}?via=${encodeURIComponent(via)}` : `/views/${id}`;
  const viewHref = link.child_view_id ? viewPath(link.child_view_id) : null;
  const first = link.first_unread;
  const startHref =
    first && link.child_view_id
      ? readerUrl(first.series_slug, first.issue_slug, {
          cbl: link.child_view_id,
          via,
        })
      : null;
  const openList = () => opener.go(viewPath);
  const start = first
    ? () =>
        opener.go((id) =>
          readerUrl(first.series_slug, first.issue_slug, { cbl: id, via }),
        )
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
        {viewHref ? (
          <DropdownMenuItem asChild>
            <Link href={viewHref}>Open list</Link>
          </DropdownMenuItem>
        ) : (
          <DropdownMenuItem onSelect={openList}>Open list</DropdownMenuItem>
        )}
        <DropdownMenuItem onSelect={() => onSkip(!link.skipped)}>
          {link.skipped ? "Unskip event" : "Skip event"}
        </DropdownMenuItem>
        {canManage && onMove && (
          <DropdownMenuItem onSelect={onMove}>
            {lost ? "Place…" : "Move…"}
          </DropdownMenuItem>
        )}
        {canManage && onMoveEarlier && (
          <DropdownMenuItem onSelect={onMoveEarlier}>
            Read earlier here
          </DropdownMenuItem>
        )}
        {canManage && onMoveLater && (
          <DropdownMenuItem onSelect={onMoveLater}>
            Read later here
          </DropdownMenuItem>
        )}
        {canManage && onRemove && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={onRemove}>
              Unlink from this list…
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
        data-event-link={link.id}
        data-state={state}
        className="border-border/60 flex min-h-11 scroll-mt-[calc(var(--topbar-h)+var(--pinned-controls-h,0px)+4.5rem)] items-center gap-2 rounded-md border px-3 py-1.5"
      >
        {state === "finished" ? (
          <CircleCheck
            className={cn("size-4 shrink-0", statusToneText("success"))}
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
          {state === "finished" ? progressLabel(link) : "Skipped"}
        </span>
        {lost && <LostBadge />}
        <span className="ml-auto flex shrink-0 items-center gap-1">
          {state === "skipped" && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onSkip(false)}
              aria-label={`Unskip ${link.child_name}`}
            >
              Unskip
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
      onOpenList={openList}
      onStart={start}
      opening={opener.pending}
      menu={menu}
      via={via}
    />
  );
}

/** The issue this event followed left the list in a refresh. */
function LostBadge() {
  return (
    <Badge
      variant="outline"
      className="shrink-0 gap-1"
      title="A refresh removed the issue this event followed. Place it again from its menu."
    >
      <TriangleAlert className="size-3" aria-hidden />
      Placement lost
    </Badge>
  );
}

/** Marks the band `data-in-view` once most of it is on screen, which
 *  fans its cover stack out (globals.css). Scroll, not hover, so it
 *  plays on touch too; it stays fanned while the band is in view and
 *  closes when it leaves. A callback ref with cleanup (React 19). */
function useFanInView<T extends HTMLElement>() {
  return React.useCallback((el: T | null) => {
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      ([entry]) => {
        if (entry?.isIntersecting) el.dataset.inView = "";
        else delete el.dataset.inView;
      },
      { threshold: 0.6 },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);
}

function EventBandFull({
  link,
  afterLabel,
  lost,
  percent,
  startHref,
  viewHref,
  onOpenList,
  onStart,
  opening,
  menu,
  via,
}: {
  link: ReadingListLinkView;
  afterLabel: string | null;
  lost: boolean;
  percent: number;
  /** Links when the reader already has a view; else the callbacks. */
  startHref: string | null;
  viewHref: string | null;
  onOpenList: () => void;
  onStart: (() => void) | null;
  opening: boolean;
  menu: React.ReactNode;
  /** The master's view id, carried as `?via=`. */
  via: string | null;
}) {
  const covers = useCblListEntriesInfinite(link.child_list_id, {
    limit: EVENT_COVER_LIMIT,
  });
  const items = covers.data?.pages[0]?.items ?? [];
  const owned = items.filter((i) => !!i.issue?.cover_url);
  const coverUrls = owned.map((i) => i.issue!.cover_url!);
  const lead = owned[0]?.issue ?? null;
  const tone = decodeTone(lead?.cover_tone);
  const fanRef = useFanInView<HTMLElement>();
  const started = link.read > 0;
  // "[Marvel] Age of Ultron (WEB-CBRO)" → "Age of Ultron"; the full name
  // stays in the tooltip and the accessible label.
  const title = displayListName(link.child_name);
  const more = link.total - items.length;

  return (
    <section
      ref={fanRef}
      data-testid="event-band"
      data-event-link={link.id}
      data-state="full"
      aria-label={`Event: ${link.child_name}`}
      className="event-band cover-stack-host border-border bg-card relative isolate scroll-mt-[calc(var(--topbar-h)+var(--pinned-controls-h,0px)+4.5rem)] overflow-hidden rounded-xl border"
      style={
        tone
          ? ({
              "--event-tint": `rgb(${tone.vibrant})`,
              "--event-base": `rgb(${tone.average})`,
            } as React.CSSProperties)
          : undefined
      }
    >
      {/* The lead cover, blurred into a backdrop. Decorative: the same
          thumbnail the stack's front cover already loaded. */}
      {lead?.cover_url && (
        <div
          aria-hidden
          className="event-band__backdrop"
          style={{ backgroundImage: `url("${lead.cover_url}")` }}
        />
      )}
      <div aria-hidden className="event-band__scrim" />

      <div className="grid grid-cols-[7.5rem_minmax(0,1fr)] gap-x-4 gap-y-4 p-4 sm:grid-cols-[11rem_minmax(0,1fr)] sm:gap-x-6 sm:p-5 lg:grid-cols-[15rem_minmax(0,1fr)] lg:p-6">
        <div className="self-center sm:row-span-2">
          <CoverStack urls={coverUrls} spill={false} />
        </div>

        <div className="flex min-w-0 flex-col justify-center gap-2 sm:gap-3">
          <div className="flex items-start gap-2">
            <p className="text-muted-foreground flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1 text-xs">
              <span className="event-band__eyebrow inline-flex items-center gap-1 font-semibold tracking-widest uppercase">
                <Zap className="size-3.5" aria-hidden />
                Event
              </span>
              {afterLabel && (
                <span className="min-w-0 truncate">
                  {lost ? "Was after" : "Read after"} {afterLabel}
                </span>
              )}
              {lost && <LostBadge />}
            </p>
            <div className="-mt-1.5 -mr-1.5 shrink-0">{menu}</div>
          </div>

          <h3
            title={link.child_name}
            className="line-clamp-2 text-xl leading-tight font-bold tracking-tight text-balance sm:text-2xl lg:text-3xl"
          >
            {viewHref ? (
              <Link href={viewHref} className="hover:underline">
                {title}
              </Link>
            ) : (
              <button
                type="button"
                onClick={onOpenList}
                disabled={opening}
                className="text-left hover:underline"
              >
                {title}
              </button>
            )}
          </h3>

          <p className="text-muted-foreground text-xs sm:text-sm">
            {eventLibraryLabel(link)}
          </p>

          <div className="max-w-md">
            <Progress
              value={percent}
              aria-label={`${link.child_name} progress`}
              className="h-1.5"
            />
            <p className="text-muted-foreground mt-1.5 text-xs">
              {eventProgressLabel(link)}
            </p>
          </div>

          <div className="hidden items-center gap-2 sm:flex">
            <BandActions
              started={started}
              startHref={startHref}
              onStart={onStart}
              viewHref={viewHref}
              onOpenList={onOpenList}
              opening={opening}
            />
          </div>
        </div>

        {/* Phone: the actions run the card's full width under the stack. */}
        <div className="col-span-2 grid grid-cols-2 gap-2 sm:hidden">
          <BandActions
            started={started}
            startHref={startHref}
            onStart={onStart}
            viewHref={viewHref}
            onOpenList={onOpenList}
            opening={opening}
            fill
          />
        </div>

        {items.length > 0 && (
          <ul
            role="list"
            aria-label={`First issues of ${link.child_name}`}
            className="event-band__strip col-span-2 -mx-4 flex gap-3 overflow-x-auto px-4 pb-2 sm:col-span-1 sm:col-start-2 sm:mx-0 sm:px-0"
          >
            {items.map((item) => (
              <li
                key={item.id}
                className="w-24 shrink-0 sm:w-28 lg:w-32"
                title={`${item.series_name} #${item.issue_number}`}
              >
                {item.issue ? (
                  <Link
                    href={readerUrl(item.issue, {
                      cbl: link.child_view_id,
                      via,
                    })}
                    aria-label={`Read ${item.series_name} #${item.issue_number}`}
                    className="group/strip focus-visible:ring-ring block rounded-md focus-visible:ring-2 focus-visible:outline-none"
                  >
                    <Cover
                      src={item.issue.cover_url}
                      tone={item.issue.cover_tone}
                      alt=""
                      className="aspect-[2/3] w-full rounded-md transition-transform duration-200 group-hover/strip:-translate-y-1 motion-reduce:transition-none"
                    />
                  </Link>
                ) : (
                  <div className="border-border text-muted-foreground flex aspect-[2/3] w-full items-center justify-center rounded-md border border-dashed p-2 text-center text-[11px]">
                    Not in library
                  </div>
                )}
                <p
                  className="card-title mt-1.5 text-xs font-medium"
                  title={item.series_name}
                >
                  {item.series_name}
                </p>
                <p className="text-muted-foreground truncate text-[11px]">
                  #{item.issue_number}
                </p>
              </li>
            ))}
            {more > 0 && (
              <li className="w-24 shrink-0 sm:w-28 lg:w-32">
                {viewHref ? (
                  <Link href={viewHref} className={MORE_TILE}>
                    +{more} more
                  </Link>
                ) : (
                  <button
                    type="button"
                    onClick={onOpenList}
                    disabled={opening}
                    className={MORE_TILE}
                  >
                    +{more} more
                  </button>
                )}
              </li>
            )}
          </ul>
        )}
      </div>
    </section>
  );
}

const MORE_TILE =
  "border-border bg-background/40 text-muted-foreground hover:text-foreground hover:bg-background/60 flex aspect-[2/3] w-full items-center justify-center rounded-md border text-sm font-medium transition-colors";

/** Start / Resume and Open list. `fill` stretches them (phone). */
function BandActions({
  started,
  startHref,
  onStart,
  viewHref,
  onOpenList,
  opening,
  fill = false,
}: {
  started: boolean;
  startHref: string | null;
  onStart: (() => void) | null;
  viewHref: string | null;
  onOpenList: () => void;
  opening: boolean;
  fill?: boolean;
}) {
  const label = started ? "Resume" : "Start reading";
  const cls = fill ? "w-full" : undefined;
  const hasStart = !!startHref || !!onStart;
  return (
    <>
      {startHref ? (
        <Button asChild className={cls}>
          <Link href={startHref}>
            <Play className="size-4" aria-hidden />
            {label}
          </Link>
        </Button>
      ) : onStart ? (
        <Button onClick={onStart} disabled={opening} className={cls}>
          <Play className="size-4" aria-hidden />
          {label}
        </Button>
      ) : null}
      {viewHref ? (
        <Button
          asChild
          variant="outline"
          className={cn(cls, fill && !hasStart && "col-span-2")}
        >
          <Link href={viewHref}>Open list</Link>
        </Button>
      ) : (
        <Button
          variant="outline"
          onClick={onOpenList}
          disabled={opening}
          className={cn(cls, fill && !hasStart && "col-span-2")}
        >
          Open list
        </Button>
      )}
    </>
  );
}
