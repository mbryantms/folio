"use client";

/**
 * Linked reading lists 2.0: move an event to after another issue of the
 * list, and the notice that lists events (and chapter breaks) whose issue
 * left the list in a refresh, each with the action that repairs it.
 *
 * The picker searches the list's own entries on the server (a master has
 * a thousand of them; the page may have loaded any slice), and stays open
 * until the move settles so a refusal shows where the reader is looking.
 */
import * as React from "react";
import { Loader2, MapPinOff } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useCblListEntriesInfinite } from "@/lib/api/queries";
import type {
  ReadingListBreakView,
  ReadingListLinkView,
} from "@/lib/api/types";
import { displayListName } from "@/lib/reader/list-context";

const RESULTS = 30;

export function MoveEventDialog({
  listId,
  link,
  onOpenChange,
  onPick,
  isPending,
}: {
  listId: string;
  /** The event to move; `null` = closed. */
  link: ReadingListLinkView | null;
  onOpenChange: (open: boolean) => void;
  onPick: (afterEntryId: string) => void;
  isPending: boolean;
}) {
  const [q, setQ] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  React.useEffect(() => {
    const t = setTimeout(() => setDebounced(q.trim()), 200);
    return () => clearTimeout(t);
  }, [q]);
  const open = link != null;
  const entries = useCblListEntriesInfinite(open ? listId : "", {
    q: debounced || undefined,
    limit: RESULTS,
  });
  const items = entries.data?.pages[0]?.items ?? [];
  // Keep the name on screen through the close animation.
  const [shown, setShown] = React.useState(link);
  if (link && link !== shown) setShown(link);
  const name = shown ? displayListName(shown.child_name) : "";
  const lost = shown?.anchor?.status === "lost";
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{lost ? `Place ${name}` : `Move ${name}`}</DialogTitle>
          <DialogDescription>
            Pick the issue it is read after. Search by series or number.
          </DialogDescription>
        </DialogHeader>
        <Command
          shouldFilter={false}
          className="border-border rounded-md border"
        >
          <CommandInput
            value={q}
            onValueChange={setQ}
            placeholder="Fantastic Four 8"
            aria-label="Search the issues in this list"
          />
          <CommandList className="max-h-72">
            <CommandEmpty>
              {entries.isLoading || q.trim() !== debounced
                ? "Searching…"
                : entries.isError
                  ? "Couldn't search this list. Try again."
                  : "No issue matches."}
            </CommandEmpty>
            <CommandGroup>
              {items.map((e) => (
                <CommandItem
                  key={e.id}
                  value={e.id}
                  disabled={isPending}
                  onSelect={() => onPick(e.id)}
                >
                  <span className="text-muted-foreground w-12 shrink-0 text-xs tabular-nums">
                    #{e.position + 1}
                  </span>
                  <span className="min-w-0 truncate">
                    {e.series_name} #{e.issue_number}
                  </span>
                  {e.year ? (
                    <span className="text-muted-foreground ml-auto shrink-0 text-xs">
                      {e.year}
                    </span>
                  ) : null}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
        {isPending ? (
          <p className="text-muted-foreground flex items-center gap-2 text-xs">
            <Loader2 className="size-3 animate-spin" aria-hidden /> Moving…
          </p>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

/** Events and chapter breaks whose issue left the list. Pure, for tests. */
export function lostAnchors(
  events: readonly ReadingListLinkView[],
  breaks: readonly ReadingListBreakView[],
): { events: ReadingListLinkView[]; breaks: ReadingListBreakView[] } {
  return {
    events: events.filter((e) => e.anchor?.status === "lost"),
    breaks: breaks.filter((b) => b.anchor.status === "lost"),
  };
}

/** "Was after Excalibur #12", from an anchor's stored issue. */
export function anchorWasAfter(
  anchor: { series?: string | null; number?: string | null } | null | undefined,
): string | null {
  if (!anchor?.series) return null;
  return `Was after ${anchor.series} #${anchor.number ?? ""}`.trim();
}

export function LostAnchorsNotice({
  events,
  breaks,
  canManage,
  onPlace,
  onRemoveBreak,
}: {
  events: readonly ReadingListLinkView[];
  breaks: readonly ReadingListBreakView[];
  canManage: boolean;
  onPlace: (link: ReadingListLinkView) => void;
  onRemoveBreak: (b: ReadingListBreakView) => void;
}) {
  const lost = lostAnchors(events, breaks);
  const count = lost.events.length + lost.breaks.length;
  if (count === 0) return null;
  return (
    <section
      aria-label="Events and chapters that lost their place"
      data-testid="lost-anchors-notice"
      className="border-warning/40 bg-warning/5 rounded-lg border p-3 text-sm"
    >
      <p className="flex items-center gap-2 font-medium">
        <MapPinOff className="text-warning size-4 shrink-0" aria-hidden />
        {count === 1
          ? "1 item lost its place"
          : `${count} items lost their place`}
      </p>
      <p className="text-muted-foreground mt-1 text-xs">
        A refresh removed the issue each one followed. They stay where they were
        until{" "}
        {canManage ? "you place them again" : "the list's owner places them"};
        Up Next doesn&rsquo;t offer a lost event.
      </p>
      <ul className="mt-2 space-y-1">
        {lost.events.map((e) => (
          <li key={e.id} className="flex items-center gap-2">
            <span className="min-w-0 flex-1 truncate">
              {displayListName(e.child_name)}
              {anchorWasAfter(e.anchor) ? (
                <span className="text-muted-foreground">
                  {" "}
                  · {anchorWasAfter(e.anchor)}
                </span>
              ) : null}
            </span>
            {canManage && (
              <Button
                size="sm"
                variant="outline"
                onClick={() => onPlace(e)}
                aria-label={`Place ${displayListName(e.child_name)}`}
              >
                Place…
              </Button>
            )}
          </li>
        ))}
        {lost.breaks.map((b) => (
          <li key={b.id} className="flex items-center gap-2">
            <span className="min-w-0 flex-1 truncate">
              Chapter {b.label ? `“${b.label}”` : "break"}
              {anchorWasAfter(b.anchor) ? (
                <span className="text-muted-foreground">
                  {" "}
                  ·{" "}
                  {anchorWasAfter(b.anchor)?.replace("Was after", "Started at")}
                </span>
              ) : null}
            </span>
            {canManage && (
              <Button
                size="sm"
                variant="outline"
                onClick={() => onRemoveBreak(b)}
                aria-label={`Remove chapter ${b.label ?? "break"}`}
              >
                Remove…
              </Button>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * The events sharing `link`'s anchor issue, in reading order, when there
 * is more than one (so "Move earlier / later" make sense). Pure.
 */
export function anchorSiblings(
  events: readonly ReadingListLinkView[],
  link: ReadingListLinkView,
): ReadingListLinkView[] {
  const pos = link.anchor?.position;
  if (pos == null || link.anchor?.status !== "placed") return [];
  const same = events.filter(
    (e) => e.anchor?.position === pos && e.anchor?.status === "placed",
  );
  return same.length > 1 ? same : [];
}

/**
 * The `sort_within_anchor` writes that move `link` one step `dir` among
 * its siblings: every sibling whose slot changes gets its new index, so
 * ties in the stored values can't leave the order unchanged. Pure.
 */
export function reorderWrites(
  siblings: readonly ReadingListLinkView[],
  link: ReadingListLinkView,
  dir: -1 | 1,
): { linkId: string; sort_within_anchor: number }[] {
  const i = siblings.findIndex((s) => s.id === link.id);
  const j = i + dir;
  if (i < 0 || j < 0 || j >= siblings.length) return [];
  const next = [...siblings];
  [next[i], next[j]] = [next[j]!, next[i]!];
  return next
    .map((s, index) => ({ s, index }))
    .filter(({ s, index }) => s.sort_within_anchor !== index)
    .map(({ s, index }) => ({ linkId: s.id, sort_within_anchor: index }));
}
