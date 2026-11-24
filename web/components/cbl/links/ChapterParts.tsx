"use client";

/**
 * Linked reading lists (PR 2b): the chapters a master list's event
 * anchors divide it into — the pinned header above each stretch, and the
 * rail that jumps between them (desktop beside the grid, phone in a sheet).
 */
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
import { cn } from "@/lib/utils";

/** A chapter the reader has finished: everything they own in it is read. */
export function chapterDone(c: ReadingListChapterView): boolean {
  return c.in_library > 0 && c.read >= c.in_library;
}

/** DOM id of a chapter's header, the rail's jump target. */
export function chapterAnchorId(index: number): string {
  return `cbl-chapter-${index}`;
}

export function ChapterHeader({
  chapter,
  folded,
  onToggle,
}: {
  chapter: ReadingListChapterView;
  /** `undefined` when folding is off (search / filters active). */
  folded?: boolean;
  onToggle?: () => void;
}) {
  const done = chapterDone(chapter);
  return (
    <div
      id={chapterAnchorId(chapter.index)}
      data-testid="chapter-header"
      className="bg-background/95 supports-[backdrop-filter]:bg-background/80 border-border/60 sticky top-[var(--cbl-chapter-top,3.5rem)] z-10 flex scroll-mt-28 items-center gap-2 border-b py-2 backdrop-blur"
    >
      {onToggle ? (
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={!folded}
          className="hover:text-foreground flex min-h-11 min-w-0 items-center gap-2 text-left"
        >
          <ChevronRight
            className={cn(
              "size-4 shrink-0 transition-transform",
              !folded && "rotate-90",
            )}
            aria-hidden
          />
          <span className="truncate text-sm font-semibold">
            Chapter {chapter.index + 1} · {chapter.label}
          </span>
        </button>
      ) : (
        <span className="truncate text-sm font-semibold">
          Chapter {chapter.index + 1} · {chapter.label}
        </span>
      )}
      <span className="text-muted-foreground ml-auto flex shrink-0 items-center gap-1 text-xs">
        {done && (
          <CircleCheck
            className={cn("size-3.5", statusToneText("success"))}
            aria-hidden
          />
        )}
        {chapter.read} / {chapter.total} read
      </span>
    </div>
  );
}

function RailBody({
  chapters,
  events,
  next,
  onJump,
}: {
  chapters: ReadingListChapterView[];
  events: ReadingListLinkView[];
  next: ReadingListLinkView | null;
  onJump: (index: number) => void;
}) {
  const byId = new Map(events.map((e) => [e.id, e]));
  return (
    <nav aria-label="Chapters">
      <ol className="space-y-0.5 text-sm">
        {chapters.map((c) => {
          const done = chapterDone(c);
          return (
            <li key={c.index}>
              <button
                type="button"
                onClick={() => onJump(c.index)}
                className="hover:bg-accent flex min-h-9 w-full items-center gap-2 rounded-md px-2 text-left"
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
                <span className="min-w-0 flex-1 truncate">
                  {c.index + 1}. {c.label}
                </span>
                <span className="text-muted-foreground shrink-0 text-xs">
                  {c.read}/{c.total}
                </span>
              </button>
              {c.event_link_ids.length > 0 && (
                <ul className="text-muted-foreground ml-6 space-y-0.5 text-xs">
                  {c.event_link_ids.map((id) => {
                    const ev = byId.get(id);
                    if (!ev) return null;
                    return (
                      <li key={id} className="flex items-center gap-1.5">
                        <Zap
                          className={cn("size-3", statusToneText("warning"))}
                          aria-hidden
                        />
                        <span className="truncate">{ev.child_name}</span>
                        <span className="ml-auto shrink-0">
                          {ev.skipped ? "skipped" : `${ev.read}/${ev.total}`}
                        </span>
                      </li>
                    );
                  })}
                </ul>
              )}
            </li>
          );
        })}
      </ol>
      {next && (
        <p className="text-muted-foreground mt-3 px-2 text-xs">
          Continues in {next.child_name} →
        </p>
      )}
    </nav>
  );
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
  onJump: (index: number) => void;
  onHide: () => void;
}) {
  return (
    <aside className="sticky top-20 hidden max-h-[calc(100dvh-6rem)] w-60 shrink-0 overflow-y-auto xl:block">
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
      <RailBody {...props} />
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
  onJump: (index: number) => void;
}) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="right" className="w-80 overflow-y-auto">
        <SheetHeader>
          <SheetTitle>Chapters</SheetTitle>
          <SheetDescription>
            Jump to a stretch of the list. Events are read at the end of the
            chapter they&rsquo;re listed under.
          </SheetDescription>
        </SheetHeader>
        <div className="px-4 pb-4">
          <RailBody
            {...props}
            onJump={(i) => {
              onOpenChange(false);
              props.onJump(i);
            }}
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
