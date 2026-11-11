"use client";

import type * as React from "react";

import { GRID_GAP_PX, effectiveColumnWidth } from "@/lib/library/grid-window";
import { useContainerWidth } from "@/lib/use-container-width";

/**
 * WP-7.7: the real column width of a fill-the-row cover grid
 * (`COVER_GRID_COLUMNS`) as wide as the returned ref's element, measured
 * with a `ResizeObserver` (`useContainerWidth`). `minSize` is the shared
 * cover target (`useCoverTarget`). Attach the ref to a container as wide
 * as the grid being matched (on the series page, the Related tab panel
 * spans the same column as the Issues grid), and size each cover with the
 * returned width so it matches the grid exactly and follows the size
 * slider live. Returns `minSize` until the first measurement.
 */
export function useGridColumnWidth<E extends HTMLElement = HTMLDivElement>(
  minSize: number,
  gap: number = GRID_GAP_PX,
): [React.RefObject<E | null>, number] {
  const [ref, width] = useContainerWidth<E>();
  return [ref, effectiveColumnWidth(width, minSize, gap)];
}
