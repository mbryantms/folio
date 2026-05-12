"use client";

import * as React from "react";

import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { useCardSize } from "@/components/library/use-card-size";
import { Button } from "@/components/ui/button";
import { useSavedViewResultsInfinite } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

import { EditFilterViewSheet } from "./EditFilterViewSheet";
import { ViewHeader } from "./ViewHeader";

/** Saved-view-page card-size bounds. Mirrors the series Issues panel
 *  but stored under a separate localStorage key so users can tune the
 *  saved-view density independently. */
const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.savedView.cardSize";

/** Filter-view detail page: header + paginated series grid. The
 *  cursor-paginated results endpoint can return up to 200 rows per
 *  page; the "Load more" button below pulls additional pages until
 *  the server stops returning a `next_cursor`. */
export function FilterViewDetail({ view }: { view: SavedViewView }) {
  const [editOpen, setEditOpen] = React.useState(false);
  const results = useSavedViewResultsInfinite(view.id);
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  const items = (results.data?.pages ?? []).flatMap((p) => p.items);
  const isInitialLoading = results.isLoading;

  // Auto-fill grid driven by the slider — column count adapts as the
  // user drags. Falls back to the default px while localStorage
  // rehydrates so the SSR markup stays stable.
  const gridStyle: React.CSSProperties = {
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  };

  return (
    <div className="space-y-6">
      <ViewHeader
        view={view}
        onEdit={() => setEditOpen(true)}
        extraActions={
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
          />
        }
      />

      {isInitialLoading ? (
        <SeriesGridSkeleton style={gridStyle} />
      ) : items.length === 0 ? (
        <EmptyState />
      ) : (
        <ul role="list" className="grid gap-4" style={gridStyle}>
          {items.map((s) => (
            <li key={s.id}>
              <SeriesCard series={s} size="md" />
            </li>
          ))}
        </ul>
      )}

      {results.hasNextPage ? (
        <div className="flex justify-center">
          <Button
            type="button"
            variant="outline"
            onClick={() => results.fetchNextPage()}
            disabled={results.isFetchingNextPage}
          >
            {results.isFetchingNextPage ? "Loading…" : "Load more"}
          </Button>
        </div>
      ) : null}

      <EditFilterViewSheet
        view={view}
        open={editOpen}
        onOpenChange={setEditOpen}
      />
    </div>
  );
}

function SeriesGridSkeleton({ style }: { style: React.CSSProperties }) {
  return (
    <ul role="list" className="grid gap-4" style={style}>
      {Array.from({ length: 12 }).map((_, i) => (
        <li key={i}>
          <SeriesCardSkeleton size="md" />
        </li>
      ))}
    </ul>
  );
}

function EmptyState() {
  return (
    <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
      Nothing matches this view yet. Tweak the conditions to broaden the search.
    </div>
  );
}
