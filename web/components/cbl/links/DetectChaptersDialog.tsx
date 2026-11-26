"use client";

/**
 * Linked reading lists 2.0: "Find event chapters…". For a list that
 * already has its events inline (a CMRO part, a with-events master), the
 * server finds each event read as one stretch and the reader picks which
 * get a named chapter. Applying creates ordinary chapter breaks: one at
 * the event's first issue, named after it, and one where the list goes
 * on after it, so the event is its own chapter.
 *
 * Opt-in and per list: nothing is added on import, and a reader who
 * wants no chapters uses the list's Plain view.
 */
import * as React from "react";
import { Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  useApplyDetectedChapters,
  useDetectCblChapters,
  type DetectedBreak,
} from "@/lib/api/mutations/reading-list-links";
import type { DetectChaptersPreview, ProposedChapter } from "@/lib/api/types";
import { displayListName } from "@/lib/reader/list-context";

/** A chapter named after its event: "Event: Age of Ultron". */
export function chapterLabel(p: ProposedChapter): string {
  return `Event: ${displayListName(p.name)}`.slice(0, 120);
}

/**
 * The breaks to create for the picked proposals, in reading order: the
 * event's start (named), then the issue after it (unnamed, so that
 * chapter is named after its first issue), skipping any place a chapter
 * already starts, including one this same batch creates. Pure.
 */
export function detectedBreaks(
  proposals: readonly ProposedChapter[],
  picked: ReadonlySet<string>,
): DetectedBreak[] {
  const out: DetectedBreak[] = [];
  const taken = new Set<string>();
  for (const p of [...proposals].sort((a, b) => a.position - b.position)) {
    if (!picked.has(p.entry_id)) continue;
    if (!p.already_chapter && !taken.has(p.entry_id)) {
      out.push({ entry_id: p.entry_id, label: chapterLabel(p) });
      taken.add(p.entry_id);
    }
    if (
      p.next_entry_id &&
      !p.next_already_chapter &&
      !taken.has(p.next_entry_id) &&
      // The next event starts right there: its own named break covers it.
      !proposals.some(
        (q) => q.entry_id === p.next_entry_id && picked.has(q.entry_id),
      )
    ) {
      out.push({ entry_id: p.next_entry_id, label: null });
      taken.add(p.next_entry_id);
    }
  }
  return out;
}

export function DetectChaptersDialog({
  listId,
  open,
  onOpenChange,
}: {
  listId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const detectM = useDetectCblChapters(listId);
  const applyM = useApplyDetectedChapters(listId);
  const [preview, setPreview] = React.useState<DetectChaptersPreview | null>(
    null,
  );
  const [picked, setPicked] = React.useState<Set<string>>(new Set());
  const [failed, setFailed] = React.useState(false);
  const { mutate: runDetect } = detectM;

  const fetchPreview = React.useCallback(() => {
    runDetect(undefined, {
      onSuccess: (p) => {
        if (!p) return;
        setPreview(p);
        setPicked(
          new Set(
            p.proposals
              .filter((x) => !x.already_chapter)
              .map((x) => x.entry_id),
          ),
        );
      },
      onError: () => setFailed(true),
    });
  }, [runDetect]);
  React.useEffect(() => {
    if (open) fetchPreview();
  }, [open, fetchPreview]);
  const retry = () => {
    setFailed(false);
    fetchPreview();
  };

  const breaks = preview ? detectedBreaks(preview.proposals, picked) : [];
  const count = preview
    ? preview.proposals.filter(
        (p) => !p.already_chapter && picked.has(p.entry_id),
      ).length
    : 0;
  const selectable = preview
    ? preview.proposals.filter((p) => !p.already_chapter)
    : [];
  const allPicked = selectable.every((p) => picked.has(p.entry_id));

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85dvh] flex-col sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Find event chapters</DialogTitle>
          <DialogDescription>
            Events this list already reads in place. Each one you keep becomes a
            named chapter; nothing is moved.
          </DialogDescription>
        </DialogHeader>

        <div className="-mx-6 min-h-0 flex-1 overflow-y-auto px-6">
          {preview == null ? (
            failed ? (
              <div className="space-y-3 py-6">
                <p className="text-muted-foreground text-sm">
                  Couldn&rsquo;t look for events.
                </p>
                <Button variant="outline" size="sm" onClick={retry}>
                  Try again
                </Button>
              </div>
            ) : (
              <div className="space-y-3 py-6" aria-busy="true">
                <p className="text-muted-foreground flex items-center gap-2 text-sm">
                  <Loader2 className="size-4 animate-spin" /> Comparing with
                  event lists… this can take up to half a minute.
                </p>
                {[0, 1, 2].map((i) => (
                  <div
                    key={i}
                    className="bg-muted h-9 animate-pulse rounded"
                    aria-hidden
                  />
                ))}
              </div>
            )
          ) : preview.proposals.length === 0 ? (
            <p className="text-muted-foreground py-6 text-sm">
              {preview.compared === 0
                ? "There were no event lists to compare with. Import the events you want as chapters (or use a list from the catalog) and try again."
                : `None of the ${preview.compared} event lists compared is read here as one stretch.`}
            </p>
          ) : (
            <section className="space-y-2 py-1">
              <div className="flex items-center justify-between gap-2">
                <h3 className="text-sm font-medium">
                  {preview.proposals.length}{" "}
                  {preview.proposals.length === 1 ? "event" : "events"} found
                </h3>
                {selectable.length > 1 && (
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={applyM.isPending}
                    onClick={() =>
                      setPicked(
                        allPicked
                          ? new Set()
                          : new Set(selectable.map((p) => p.entry_id)),
                      )
                    }
                  >
                    {allPicked ? "Select none" : "Select all"}
                  </Button>
                )}
              </div>
              <ul className="space-y-1">
                {preview.proposals.map((p) => {
                  const id = `detect-${p.entry_id}`;
                  return (
                    <li
                      key={p.entry_id}
                      className="flex items-start gap-3 py-1"
                    >
                      <Checkbox
                        id={id}
                        className="mt-0.5"
                        checked={p.already_chapter || picked.has(p.entry_id)}
                        disabled={p.already_chapter || applyM.isPending}
                        onCheckedChange={(v) =>
                          setPicked((prev) => {
                            const next = new Set(prev);
                            if (v === true) next.add(p.entry_id);
                            else next.delete(p.entry_id);
                            return next;
                          })
                        }
                      />
                      <label htmlFor={id} className="min-w-0 text-sm">
                        <span
                          className="line-clamp-2 font-medium"
                          title={p.name}
                        >
                          {displayListName(p.name)}
                        </span>
                        <span className="text-muted-foreground block text-xs">
                          Starts at {p.starts_at} (#{p.position + 1}) ·{" "}
                          {p.issues} of its {p.total} issues here
                          {p.already_chapter ? " · already a chapter" : ""}
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            </section>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {count === 0 ? "Close" : "Cancel"}
          </Button>
          {count > 0 && (
            <Button
              disabled={applyM.isPending}
              onClick={() =>
                applyM.mutate(breaks, { onSuccess: () => onOpenChange(false) })
              }
            >
              {applyM.isPending && <Loader2 className="size-4 animate-spin" />}
              Add {count} {count === 1 ? "chapter" : "chapters"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
