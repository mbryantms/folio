"use client";

/**
 * Linked reading lists (PR 2b): the chapters a master list's event
 * anchors divide it into — the pinned header above each stretch, and the
 * rail that jumps between them (desktop beside the grid, phone in a sheet).
 */
import * as React from "react";
import {
  ChevronRight,
  CircleCheck,
  ListTree,
  PanelRightClose,
  PanelRightOpen,
  Zap,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/folio/sheet";
import type {
  ReadingListChapterView,
  ReadingListLinkView,
} from "@/lib/api/types";
import { statusToneDot, statusToneText } from "@/lib/ui/status-tone";
import { progressLabel, progressShort } from "@/lib/cbl/linked-plan";
import { displayListName } from "@/lib/reader/list-context";
import { useCurrentChapter } from "./use-current-chapter";
import { cn } from "@/lib/utils";

/** A chapter the reader has finished: everything they own in it is read. */
export function chapterDone(c: ReadingListChapterView): boolean {
  return c.in_library > 0 && c.read >= c.in_library;
}

/** A chapter's label for display. A chapter the server named after its
 *  event ("Up to [Marvel] Age of Ultron (WEB-CBRO)") drops the catalog
 *  tags the same way list names do; a name the reader typed is kept. */
export function chapterDisplayLabel(label: string): string {
  const m = /^Up to (.+)$/.exec(label);
  return m ? `Up to ${displayListName(m[1]!)}` : label;
}

/** DOM id of a chapter's header, the rail's jump target. */
export function chapterAnchorId(index: number): string {
  return `cbl-chapter-${index}`;
}

export function ChapterHeader({
  chapter,
  folded,
  onToggle,
  actions,
}: {
  chapter: ReadingListChapterView;
  /** `undefined` when folding is off (search / filters active). */
  folded?: boolean;
  onToggle?: () => void;
  /** Trailing controls (the chapter-break menu, for managers). */
  actions?: React.ReactNode;
}) {
  const done = chapterDone(chapter);
  return (
    <div
      id={chapterAnchorId(chapter.index)}
      data-testid="chapter-header"
      className="bg-background/95 supports-[backdrop-filter]:bg-background/80 border-border/60 sticky top-[calc(var(--topbar-h)+var(--pinned-controls-h,0px))] z-10 flex items-center gap-2 border-b py-2 backdrop-blur"
    >
      {/* A heading, so chapters are reachable by heading navigation. */}
      <h2 className="flex min-w-0 text-sm font-semibold">
        {onToggle ? (
          <button
            type="button"
            onClick={onToggle}
            aria-expanded={!folded}
            className="hover:text-foreground flex min-h-11 min-w-0 items-center gap-2 text-left"
          >
            <ChevronRight
              className={cn(
                "size-4 shrink-0 transition-transform motion-reduce:transition-none",
                !folded && "rotate-90",
              )}
              aria-hidden
            />
            <span className="truncate">
              Chapter {chapter.index + 1} · {chapterDisplayLabel(chapter.label)}
            </span>
          </button>
        ) : (
          <span className="truncate">
            Chapter {chapter.index + 1} · {chapterDisplayLabel(chapter.label)}
          </span>
        )}
      </h2>
      <span className="text-muted-foreground ml-auto flex shrink-0 items-center gap-1 text-xs">
        {done && (
          <CircleCheck
            className={cn("size-3.5", statusToneText("success"))}
            aria-hidden
          />
        )}
        {progressLabel(chapter)}
      </span>
      {actions}
    </div>
  );
}

/** Data attribute on the element that scrolls the rail (the desktop
 *  column or the sheet), so the current chapter can be kept in view
 *  inside it without scrolling the page. */
const RAIL_SCROLL_ATTR = "data-chapter-rail-scroll";

/** The chapter list itself, with the current chapter marked. Exported
 *  for tests; pages use {@link ChapterRail} / {@link ChapterRailSheet}. */
export function ChapterRailList({
  chapters,
  events,
  next,
  current,
  onJump,
  onJumpEvent,
}: {
  chapters: ReadingListChapterView[];
  events: ReadingListLinkView[];
  next: ReadingListLinkView | null;
  /** The chapter in view (`useCurrentChapter`). */
  current: number;
  onJump: (index: number) => void;
  /** Jump to an event's band (in the chapter it is listed under). */
  onJumpEvent?: (linkId: string, chapterIndex: number) => void;
}) {
  const byId = new Map(events.map((e) => [e.id, e]));
  const listRef = React.useRef<HTMLDivElement>(null);
  const barRef = React.useRef<HTMLSpanElement>(null);
  // Slide the indicator to the current chapter's row. Written to the DOM
  // directly: it follows the page's scroll, which shouldn't re-render the
  // list. The first placement doesn't animate (no slide in from the top).
  React.useLayoutEffect(() => {
    const list = listRef.current;
    const bar = barRef.current;
    if (!list || !bar) return;
    const row = list.querySelector<HTMLElement>(
      `[data-rail-chapter="${current}"]`,
    );
    if (!row) {
      bar.style.opacity = "0";
      return;
    }
    bar.style.transform = `translateY(${row.offsetTop}px)`;
    bar.style.height = `${row.offsetHeight}px`;
    bar.style.opacity = "1";
    if (!bar.dataset.placed) {
      // Let this position paint before transitions switch on.
      requestAnimationFrame(() => {
        if (barRef.current) barRef.current.dataset.placed = "true";
      });
    }
    // Keep the row in view inside the rail, never scrolling the page.
    const scroller = list.closest<HTMLElement>(`[${RAIL_SCROLL_ATTR}]`);
    if (scroller) {
      const box = scroller.getBoundingClientRect();
      const r = row.getBoundingClientRect();
      const margin = 48;
      if (r.top < box.top + margin || r.bottom > box.bottom - margin) {
        scroller.scrollTop += r.top - box.top - box.height / 2 + r.height / 2;
      }
    }
  }, [current, chapters]);
  return (
    <nav aria-label="Chapters">
      <div ref={listRef} className="relative pl-2">
        <span
          ref={barRef}
          aria-hidden
          className="bg-primary pointer-events-none absolute top-0 left-0 w-0.5 rounded-full opacity-0 data-[placed]:transition-[transform,height] data-[placed]:duration-300 data-[placed]:ease-out motion-reduce:transition-none"
        />
        <ol className="space-y-0.5 text-sm">
          {chapters.map((c) => {
            const done = chapterDone(c);
            const isCurrent = c.index === current;
            return (
              <li key={c.index}>
                <button
                  type="button"
                  data-rail-chapter={c.index}
                  aria-current={isCurrent ? "location" : undefined}
                  onClick={() => onJump(c.index)}
                  className={cn(
                    "folio-tap hover:bg-accent flex min-h-9 w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors motion-reduce:transition-none",
                    isCurrent && "bg-muted text-foreground font-medium",
                  )}
                >
                  <span
                    aria-hidden
                    className={cn(
                      "size-2 shrink-0 rounded-full",
                      done
                        ? statusToneDot("success")
                        : c.read > 0
                          ? "bg-primary"
                          : "border-muted-foreground border",
                    )}
                  />
                  <span
                    className="line-clamp-2 min-w-0 flex-1 leading-snug"
                    title={c.label}
                  >
                    {c.index + 1}. {chapterDisplayLabel(c.label)}
                  </span>
                  <span className="text-muted-foreground shrink-0 text-xs">
                    <span title={progressLabel(c)}>{progressShort(c)}</span>
                  </span>
                </button>
                {c.event_link_ids.length > 0 && (
                  <ul className="text-muted-foreground ml-6 space-y-0.5 text-xs">
                    {c.event_link_ids.map((id) => {
                      const ev = byId.get(id);
                      if (!ev) return null;
                      const name = displayListName(ev.child_name);
                      const body = (
                        <>
                          <Zap
                            className={cn(
                              "size-3 shrink-0",
                              statusToneText("warning"),
                            )}
                            aria-hidden
                          />
                          <span className="truncate" title={ev.child_name}>
                            {name}
                          </span>
                          <span className="ml-auto shrink-0">
                            {ev.skipped ? "skipped" : progressShort(ev)}
                          </span>
                        </>
                      );
                      return (
                        <li key={id}>
                          {onJumpEvent ? (
                            <button
                              type="button"
                              onClick={() => onJumpEvent(id, c.index)}
                              aria-label={`Go to event ${name}`}
                              className="hover:bg-accent hover:text-foreground flex min-h-7 w-full items-center gap-1.5 rounded px-1 text-left"
                            >
                              {body}
                            </button>
                          ) : (
                            <span className="flex items-center gap-1.5 px-1">
                              {body}
                            </span>
                          )}
                        </li>
                      );
                    })}
                  </ul>
                )}
              </li>
            );
          })}
        </ol>
      </div>
      {next && (
        <p className="text-muted-foreground mt-3 px-2 text-xs">
          Continues in {displayListName(next.child_name)} →
        </p>
      )}
    </nav>
  );
}

/** The list wired to the page's scroll position. The tracking state
 *  lives here, not in the page: crossing a chapter re-renders the rail
 *  only, never the cover grid. */
function TrackedRailList({
  jumpTarget,
  ...props
}: Omit<React.ComponentProps<typeof ChapterRailList>, "current"> & {
  jumpTarget: number | null;
}) {
  const current = useCurrentChapter(props.chapters.length, jumpTarget);
  return <ChapterRailList {...props} current={current} />;
}

/** Desktop: a sticky column beside the grid, which the reader can hide
 *  (then the toolbar's Chapters button opens it in a sheet instead). */
export function ChapterRail({
  onHide,
  ...props
}: {
  chapters: ReadingListChapterView[];
  events: ReadingListLinkView[];
  next: ReadingListLinkView | null;
  /** The chapter a jump is heading for, while it is under way. */
  jumpTarget: number | null;
  onJump: (index: number) => void;
  onJumpEvent?: (linkId: string, chapterIndex: number) => void;
  onHide: () => void;
}) {
  return (
    <aside
      {...{ [RAIL_SCROLL_ATTR]: "" }}
      className="sticky top-[calc(var(--topbar-h)+var(--pinned-controls-h,0px)+1rem)] hidden max-h-[calc(100dvh-var(--topbar-h)-var(--pinned-controls-h,0px)-2rem)] w-60 shrink-0 overflow-y-auto xl:block"
    >
      <div className="mb-1 flex items-center justify-between pl-2">
        <p className="text-muted-foreground text-xs font-semibold tracking-wide uppercase">
          Chapters
        </p>
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={onHide}
          aria-label="Hide chapters"
          title="Hide chapters"
        >
          <PanelRightClose className="size-4" aria-hidden />
        </Button>
      </div>
      <TrackedRailList {...props} />
    </aside>
  );
}

/** Below `xl`: the rail in a sheet, opened from the toolbar's Chapters
 *  button (`sm`+) or the phone bar's menu. Controlled so both entry
 *  points share one sheet. */
export function ChapterRailSheet({
  open,
  onOpenChange,
  onShowRail,
  ...props
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Bring the hidden desktop rail back; offered at `xl` only. */
  onShowRail?: () => void;
  chapters: ReadingListChapterView[];
  events: ReadingListLinkView[];
  next: ReadingListLinkView | null;
  jumpTarget: number | null;
  onJump: (index: number) => void;
  onJumpEvent?: (linkId: string, chapterIndex: number) => void;
}) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        // Up and down only. `overflow-y-auto` alone computes
        // `overflow-x: auto` too, and a phone could then pan the sheet
        // sideways, sliding the current-chapter bar off its left edge.
        className="w-80 touch-pan-y overflow-x-hidden overflow-y-auto overscroll-contain"
        {...{ [RAIL_SCROLL_ATTR]: "" }}
      >
        <SheetHeader>
          <SheetTitle>Chapters</SheetTitle>
          <SheetDescription>
            Jump to a stretch of the list. Events are read at the end of the
            chapter they&rsquo;re listed under.
          </SheetDescription>
        </SheetHeader>
        <div className="px-2 pb-4">
          <TrackedRailList
            {...props}
            onJump={(i) => {
              onOpenChange(false);
              props.onJump(i);
            }}
            onJumpEvent={
              props.onJumpEvent
                ? (id, i) => {
                    onOpenChange(false);
                    props.onJumpEvent?.(id, i);
                  }
                : undefined
            }
          />
          {onShowRail && (
            <Button
              variant="outline"
              size="sm"
              className="mt-4 hidden xl:inline-flex"
              onClick={() => {
                onOpenChange(false);
                onShowRail();
              }}
            >
              <PanelRightOpen className="size-4" aria-hidden /> Show beside the
              list
            </Button>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}

/** The toolbar button that opens {@link ChapterRailSheet}: `sm`–`xl`,
 *  and at `xl` too once the reader hid the rail. */
export function ChapterRailButton({
  onClick,
  railHidden,
}: {
  onClick: () => void;
  railHidden: boolean;
}) {
  return (
    <Button
      variant="outline"
      size="sm"
      className={railHidden ? undefined : "xl:hidden"}
      onClick={onClick}
    >
      <ListTree className="size-4" aria-hidden /> Chapters
    </Button>
  );
}
