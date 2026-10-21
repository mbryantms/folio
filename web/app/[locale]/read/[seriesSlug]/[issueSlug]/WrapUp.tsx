"use client";

/**
 * The two "Wrap up" pieces shared by the end-of-issue card (single /
 * double view) and the webtoon end footer:
 *
 *  - `<WrapUpStrip>` — one compact row: inline stars + "Rate" / "4/5"
 *    on the left, a "Wrap up" button with a marker-count badge on the
 *    right. Sits at the very bottom of the Up Next surface so the
 *    next-issue flow above it is untouched.
 *  - `<WrapUpBody>` — the full pane: larger stars, the issue-level
 *    closing note, and the markers-in-this-issue row with Review.
 *
 * Both rows obey the fit rule from the toolbar / widget passes: text
 * truncates or takes its own line, controls never shrink, nothing can
 * slide under a neighbour at the card's 320px width.
 */

import { ChevronRight, ListOrdered } from "lucide-react";

import { ClosingNoteEditor } from "@/components/library/ClosingNoteEditor";
import { Button } from "@/components/ui/button";
import { RatingStars } from "@/components/ui/rating-stars";
import {
  markerBreakdown,
  markerTotalLabel,
  type WrapUpProps,
} from "@/lib/reader/wrap-up";
import { cn } from "@/lib/utils";

export function WrapUpStrip({
  wrapUp,
  onOpen,
  className,
}: {
  wrapUp: WrapUpProps;
  onOpen: () => void;
  className?: string;
}) {
  const { rating, markerCounts } = wrapUp;
  return (
    <div
      data-wrap-up-strip=""
      className={cn(
        "bg-muted/40 flex items-center justify-between gap-1 overflow-hidden py-2 pr-2 pl-3",
        className,
      )}
    >
      <div className="flex min-w-0 items-center gap-1.5">
        <RatingStars
          value={rating}
          onChange={wrapUp.onRate}
          size="sm"
          label={`Rate ${wrapUp.issueLabel}`}
          className="shrink-0"
        />
        <span className="text-muted-foreground shrink-0 text-xs whitespace-nowrap tabular-nums">
          {rating !== null && rating > 0 ? `${rating}/5` : "Rate"}
        </span>
      </div>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        onClick={onOpen}
        className="h-8 shrink-0 gap-1 px-1.5 whitespace-nowrap"
      >
        Wrap up
        {markerCounts.total > 0 ? (
          <span
            className="bg-muted text-muted-foreground inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-full px-1.5 text-[11px]"
            aria-label={`${markerCounts.total} markers in this issue`}
          >
            {markerCounts.total}
          </span>
        ) : null}
        <ChevronRight className="h-4 w-4" aria-hidden="true" />
      </Button>
    </div>
  );
}

export function WrapUpBody({
  wrapUp,
  noteId = "closing-note",
}: {
  wrapUp: WrapUpProps;
  /** Unique textarea id when two bodies can be mounted at once. */
  noteId?: string;
}) {
  const { rating, markerCounts } = wrapUp;
  const breakdown = markerBreakdown(markerCounts);
  return (
    <div className="flex flex-col">
      <div className="space-y-1.5 px-4 pt-1 pb-3">
        <div className="flex items-baseline justify-between gap-2">
          <span className="text-muted-foreground text-xs font-medium">
            Your rating
          </span>
          <span className="text-muted-foreground text-xs tabular-nums">
            {rating !== null && rating > 0 ? `${rating}/5` : "Not rated"}
          </span>
        </div>
        <RatingStars
          value={rating}
          onChange={wrapUp.onRate}
          size="lg"
          label={`Rate ${wrapUp.issueLabel}`}
        />
      </div>

      <div className="border-border border-t px-4 py-3">
        <ClosingNoteEditor
          id={noteId}
          value={wrapUp.note}
          onSave={wrapUp.onSaveNote}
          saving={wrapUp.noteSaving}
          variant="card"
        />
      </div>

      <div className="border-border flex items-center justify-between gap-3 border-t px-4 py-3">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-muted-foreground text-xs font-medium">
            In this issue
          </span>
          <span className="truncate text-sm">
            {markerTotalLabel(markerCounts)}
          </span>
          {breakdown ? (
            <span className="text-muted-foreground truncate text-xs">
              {breakdown}
            </span>
          ) : null}
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={wrapUp.onReviewMarkers}
          disabled={markerCounts.total === 0}
          className="h-8 shrink-0 whitespace-nowrap"
        >
          <ListOrdered className="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
          Review
        </Button>
      </div>
    </div>
  );
}
