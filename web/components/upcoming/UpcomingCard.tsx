"use client";

import { CoverChip } from "@/components/library/CoverChip";
import { CoverFrame } from "@/components/library/CoverFrame";
import { UpcomingCover } from "@/components/upcoming/UpcomingCover";
import { UpcomingStageBadge } from "@/components/upcoming/UpcomingStageBadge";
import type { UpcomingEntryView } from "@/lib/api/types";
import { coverTintStyle } from "@/lib/ui/cover-tone-style";
import { releaseDayShort } from "@/lib/upcoming/stage";

/**
 * One upcoming release on the cover grid. The whole card opens the
 * detail dialog.
 *
 * Corners (docs/dev/card-corner-conventions.md):
 *  - top-right: the stage badge, the card's one state signal. A release
 *    nobody tracks ("announced") carries none, so in "All releases" the
 *    badges mark the reader's own series.
 *  - bottom-left: the release day, this card type's context chip.
 *  - top-left, bottom-right: empty. There is nothing to select, no menu
 *    and nothing to open in the reader, and no action on Mylar.
 *  - no dimming: the covers are the point.
 *
 * The cover sits in the shared `CoverFrame`, so it tilts and shines like
 * every other card.
 */
export function UpcomingCard({
  entry,
  onOpen,
}: {
  entry: UpcomingEntryView;
  onOpen: (entry: UpcomingEntryView) => void;
}) {
  return (
    <button
      type="button"
      data-card=""
      style={coverTintStyle(entry.cover_tone)}
      data-upcoming-key={entry.key}
      aria-haspopup="dialog"
      onClick={() => onOpen(entry)}
      className="group hover:bg-accent/40 focus-visible:ring-ring flex w-full flex-col gap-2 rounded-md p-1 text-left transition-colors focus-visible:ring-2 focus-visible:outline-none"
    >
      <CoverFrame>
        <UpcomingCover
          entry={entry}
          className="transition group-hover:brightness-110"
        />
        {entry.stage === "announced" ? null : (
          <UpcomingStageBadge
            entry={entry}
            onCover
            className="absolute top-2 right-2 max-w-[calc(100%-1rem)]"
          />
        )}
        <CoverChip className="absolute bottom-2 left-2 tabular-nums">
          {releaseDayShort(entry.release_date)}
        </CoverChip>
      </CoverFrame>
      <div className="min-w-0 px-1">
        <div className="text-muted-foreground truncate text-xs font-medium">
          #{entry.number}
          {entry.publisher ? ` · ${entry.publisher}` : ""}
        </div>
        <div className="truncate text-sm font-medium">{entry.series_name}</div>
      </div>
    </button>
  );
}
