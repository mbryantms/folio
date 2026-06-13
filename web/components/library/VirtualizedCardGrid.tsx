"use client";

import * as React from "react";
import { useWindowVirtualizer } from "@tanstack/react-virtual";

import {
  GRID_GAP_PX,
  computeColumnWidth,
  computeColumnsPerRow,
  computeRowCount,
  estimateRowHeight,
  rowItemRange,
} from "@/lib/library/grid-window";
import { useGridScrollRestore } from "@/lib/library/use-grid-scroll-restore";
import { useContainerWidth } from "@/lib/use-container-width";

/**
 * Window-virtualized responsive card grid (audit G1). Reproduces the
 * `repeat(auto-fill, minmax(cardSize, 1fr))` look but only mounts the
 * rows in (and near) the viewport, so a 10k-item library no longer
 * mounts 10k cards.
 *
 * Render-agnostic: it windows over a flat `{ id }[]` and calls
 * `renderCard` per item, so the same machinery serves the Series and
 * Issues grids (and could serve IssuesPanel later) without leaking card
 * types in here. Selection stays correct across the windowing because it
 * is keyed by id over the full `items` array in the parent — only the
 * DOM is windowed, never the data.
 *
 * Page-scroll surface → `useWindowVirtualizer` with a `scrollMargin`
 * equal to the list's distance from the top of the document. Row heights
 * are measured (`measureElement`) so card/text differences self-correct;
 * `estimateRowHeight` gives a close initial guess to minimize jump.
 */
export function VirtualizedCardGrid({
  items,
  cardSize,
  estimateTextHeight,
  hasNextPage,
  isFetchingNextPage,
  fetchNextPage,
  renderCard,
  enableScrollRestore = false,
}: {
  items: ReadonlyArray<{ id: string }>;
  cardSize: number;
  /** Approximate fixed height (px) of the card's text block below the
   *  cover — used only for the initial row-height estimate. */
  estimateTextHeight: number;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  fetchNextPage: () => void;
  renderCard: (item: { id: string }) => React.ReactNode;
  /** Persist + restore window scroll across back-nav (audit B15). */
  enableScrollRestore?: boolean;
}) {
  const [containerRef, containerWidth] = useContainerWidth<HTMLDivElement>();
  const columnsPerRow = computeColumnsPerRow(containerWidth, cardSize);
  const rowCount = computeRowCount(items.length, columnsPerRow);
  const columnWidth = computeColumnWidth(containerWidth, columnsPerRow);
  const estRowHeight = estimateRowHeight(columnWidth, estimateTextHeight);

  const listRef = React.useRef<HTMLDivElement | null>(null);
  const [scrollMargin, setScrollMargin] = React.useState(0);
  // The window virtualizer positions rows in document space, so it needs
  // the list's absolute offset from the top of the document. Remeasure
  // when the width changes — chrome above the grid (toolbar/chips) can
  // reflow and shift the list, which would otherwise offset every row.
  React.useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    setScrollMargin(el.getBoundingClientRect().top + window.scrollY);
  }, [containerWidth, rowCount]);

  const virtualizer = useWindowVirtualizer({
    count: rowCount,
    estimateSize: () => estRowHeight,
    overscan: 4,
    scrollMargin,
    getItemKey: (rowIndex) => {
      const first = items[rowIndex * columnsPerRow];
      return first ? first.id : rowIndex;
    },
  });

  // The estimate depends on cardSize/width; recompute total size when
  // either changes so the scrollbar + row offsets stay correct.
  React.useEffect(() => {
    virtualizer.measure();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [columnsPerRow, estRowHeight]);

  const virtualRows = virtualizer.getVirtualItems();
  const lastRow = virtualRows[virtualRows.length - 1]?.index ?? -1;
  // Fetch the next page when the last virtual row nears the loaded end —
  // the range-model analogue of the IntersectionObserver sentinel, which
  // is unreliable once the virtualizer controls what's mounted. Still a
  // cursor-walking `useInfiniteQuery`; no client-side truncation.
  React.useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && lastRow >= rowCount - 2) {
      fetchNextPage();
    }
  }, [hasNextPage, isFetchingNextPage, lastRow, rowCount, fetchNextPage]);

  // Save + restore window scroll across back-nav (audit B15). `rowCount`
  // is the growth signal that drives the restore's page-in loop.
  useGridScrollRestore({
    enabled: enableScrollRestore,
    getTotalSize: () => virtualizer.getTotalSize(),
    growthSignal: rowCount,
    hasNextPage,
    isFetchingNextPage,
    fetchNextPage,
  });

  // Until the first width measurement we can't pick a column count;
  // render a same-shaped auto-fill skeleton so there's no flash and the
  // ResizeObserver still has an element to measure.
  if (containerWidth <= 0) {
    return (
      <div ref={containerRef}>
        <ul
          role="list"
          className="grid gap-4"
          style={{
            gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
          }}
        >
          {Array.from({ length: 12 }).map((_, i) => (
            <li key={i}>
              <div className="bg-muted aspect-[2/3] w-full animate-pulse rounded-md" />
            </li>
          ))}
        </ul>
      </div>
    );
  }

  return (
    <div ref={containerRef}>
      <div
        ref={listRef}
        style={{
          height: `${virtualizer.getTotalSize()}px`,
          position: "relative",
          width: "100%",
        }}
      >
        {virtualRows.map((vr) => {
          const { start, end } = rowItemRange(
            vr.index,
            columnsPerRow,
            items.length,
          );
          return (
            <div
              key={vr.key}
              data-index={vr.index}
              ref={virtualizer.measureElement}
              role="list"
              className="grid"
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${vr.start - virtualizer.options.scrollMargin}px)`,
                gridTemplateColumns: `repeat(${columnsPerRow}, minmax(0, 1fr))`,
                gap: `${GRID_GAP_PX}px`,
                paddingBottom: `${GRID_GAP_PX}px`,
              }}
            >
              {items.slice(start, end).map((item) => (
                <div key={item.id}>{renderCard(item)}</div>
              ))}
            </div>
          );
        })}
      </div>
      {isFetchingNextPage ? (
        <p
          role="status"
          className="text-muted-foreground mt-2 text-center text-xs"
        >
          Loading more…
        </p>
      ) : null}
    </div>
  );
}
