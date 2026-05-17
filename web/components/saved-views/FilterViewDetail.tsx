"use client";

import * as React from "react";
import { BookOpenCheck, BookOpen, FolderPlus, ListChecks } from "lucide-react";

import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import {
  SeriesCard,
  SeriesCardSkeleton,
} from "@/components/library/SeriesCard";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { useCardSize } from "@/components/library/use-card-size";
import { Button } from "@/components/ui/button";
import { useBulkMarkSeriesProgress } from "@/lib/api/mutations";
import { useSavedViewResultsInfinite } from "@/lib/api/queries";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useSelection } from "@/lib/selection/use-selection";
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

  // Multi-select M6 extension: filter views also expose mark-read /
  // mark-unread per user request 2026-05-17. Each selected series is
  // expanded server-side to its active issues via `series-bulk`, so
  // the semantics are "mark every issue in these series read/unread"
  // — same shape as the per-series "Mark series as read" action.
  const selection = useSelection(items);
  const bulkMark = useBulkMarkSeriesProgress();
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const selectButtonRef = React.useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = React.useRef(false);
  // Esc exits select mode; Cmd/Ctrl+A selects every loaded card.
  // `shouldSkipHotkey` keeps the bindings dormant while focus is in
  // a form field.
  React.useEffect(() => {
    if (!selection.selectMode) return;
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        selection.exit();
      } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        selection.selectAll();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selection]);
  // Restore focus to the Select trigger after leaving select mode.
  React.useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);
  const selectedTargets = Array.from(selection.selected).map((id) => ({
    entry_kind: "series" as const,
    ref_id: id,
  }));

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
          <>
            <CardSizeOptions
              cardSize={cardSize}
              onCardSize={setCardSize}
              min={CARD_SIZE_MIN}
              max={CARD_SIZE_MAX}
              step={CARD_SIZE_STEP}
              defaultSize={CARD_SIZE_DEFAULT}
            />
            {!selection.selectMode && items.length > 0 && (
              <Button
                ref={selectButtonRef}
                variant="outline"
                size="sm"
                onClick={() => selection.enter()}
                aria-label="Enter select mode"
              >
                <ListChecks className="mr-1.5 h-4 w-4" />
                Select
              </Button>
            )}
          </>
        }
      />

      {selection.selectMode && (
        <SelectionToolbar
          count={selection.count}
          total={items.length}
          primary={[
            {
              id: "mark-read",
              label: "Mark read",
              icon: BookOpenCheck,
              onClick: () => {
                const series_ids = Array.from(selection.selected);
                if (series_ids.length === 0) return;
                bulkMark.mutate(
                  { series_ids, finished: true },
                  { onSuccess: () => selection.exit() },
                );
              },
              disabled: bulkMark.isPending || selection.count === 0,
            },
            {
              id: "mark-unread",
              label: "Mark unread",
              icon: BookOpen,
              onClick: () => {
                const series_ids = Array.from(selection.selected);
                if (series_ids.length === 0) return;
                bulkMark.mutate(
                  { series_ids, finished: false },
                  { onSuccess: () => selection.exit() },
                );
              },
              disabled: bulkMark.isPending || selection.count === 0,
            },
          ]}
          overflow={[
            {
              id: "add-to-collection",
              label: "Add to collection…",
              icon: FolderPlus,
              onClick: () => setPickerOpen(true),
              disabled: selection.count === 0,
            },
          ]}
          onDone={() => selection.exit()}
          onClear={() => selection.clear()}
          onSelectAll={() => selection.selectAll()}
        />
      )}

      {isInitialLoading ? (
        <SeriesGridSkeleton style={gridStyle} />
      ) : items.length === 0 ? (
        <EmptyState />
      ) : (
        <ul role="list" className="grid gap-4" style={gridStyle}>
          {items.map((s) => (
            <li key={s.id}>
              <SeriesCard
                series={s}
                size="md"
                selectMode={
                  selection.selectMode
                    ? {
                        isActive: true,
                        isSelected: selection.isSelected(s.id),
                        onToggle: (ev) => selection.toggle(s.id, ev),
                      }
                    : undefined
                }
                onEnterSelectMode={(id) => selection.toggle(id)}
              />
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

      <BulkAddToCollectionDialog
        open={pickerOpen}
        onOpenChange={(next) => {
          setPickerOpen(next);
          if (!next) selection.clear();
        }}
        targets={selectedTargets}
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
