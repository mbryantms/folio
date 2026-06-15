"use client";

import * as React from "react";
import dynamic from "next/dynamic";
import { Sparkles } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { useMe } from "@/lib/api/queries";

// Heavy match dialog — lazy so it stays out of the grid's initial bundle
// (G6); the chunk loads when the operator starts the worklist.
const MetadataMatchDialog = dynamic(
  () =>
    import("@/components/library/MetadataMatchDialog").then(
      (m) => m.MetadataMatchDialog,
    ),
  { ssr: false },
);

export type WorklistSeries = {
  seriesSlug: string;
  libraryId: string;
  name: string;
};

/** Advance the worklist cursor after an apply lands. Returns the next
 *  index, or `done` once the snapshot is exhausted (the last item's apply
 *  finishes the run rather than indexing past the end). Pure so the
 *  off-by-one at the boundary is unit-testable. */
export function nextWorklistIndex(
  queueLength: number,
  index: number,
): { index: number; done: boolean } {
  const next = index + 1;
  if (next >= queueLength) return { index: 0, done: true };
  return { index: next, done: false };
}

/**
 * "Fix metadata" CTA for the needs-metadata worklist grid (B4 part 2).
 *
 * Captures the currently-loaded needs_metadata series as an ordered queue
 * and drives the match dialog through them one at a time: applying
 * metadata to a series fires the dialog's `onApplied`, which advances to
 * the next series instead of closing — auto-advance, so the operator
 * churns the queue without re-opening the dialog per series. The dialog is
 * remounted per series via a `key` so each starts with clean search /
 * candidate state.
 *
 * The queue is a snapshot taken at start; advancing by index avoids drift
 * as applied series drop out of the live grid behind the dialog. It covers
 * the *loaded* set (what the user has scrolled into view) — not silently
 * the whole library; the count in the label reflects exactly that.
 *
 * Admin-only: the apply mutation is admin-gated, so non-admins never see
 * the button (they can still browse the filtered worklist).
 */
export function MetadataWorklistButton({
  series,
}: {
  series: WorklistSeries[];
}) {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const [queue, setQueue] = React.useState<WorklistSeries[] | null>(null);
  const [index, setIndex] = React.useState(0);

  const finish = React.useCallback((appliedCount: number) => {
    setQueue(null);
    setIndex(0);
    if (appliedCount > 0) {
      toast.success(`Updated ${appliedCount} series in the worklist`);
    }
  }, []);

  // Fired once an apply lands. Move to the next series, or wrap up when the
  // snapshot is exhausted. Identity changes with `index`, but each dialog
  // instance is remounted per series (keyed on slug) so it captures the
  // matching closure for its position.
  const advance = React.useCallback(() => {
    if (!queue) return;
    const step = nextWorklistIndex(queue.length, index);
    if (step.done) {
      finish(queue.length);
      return;
    }
    setIndex(step.index);
    const remaining = queue.length - step.index;
    toast.success(`Applied — ${remaining} more in the worklist`);
  }, [queue, index, finish]);

  if (!isAdmin || series.length === 0) return null;

  const current = queue?.[index] ?? null;

  return (
    <>
      <Button
        type="button"
        size="sm"
        className="gap-1.5"
        onClick={() => {
          setQueue(series);
          setIndex(0);
        }}
      >
        <Sparkles className="h-4 w-4" />
        Fix metadata ({series.length})
      </Button>
      {current ? (
        <MetadataMatchDialog
          key={current.seriesSlug}
          open
          onOpenChange={(next) => {
            // Dismissing mid-worklist stops it; surface what landed so far.
            if (!next) finish(index);
          }}
          scope={{
            kind: "series",
            seriesSlug: current.seriesSlug,
            libraryId: current.libraryId,
          }}
          onApplied={advance}
        />
      ) : null}
    </>
  );
}
