"use client";

import type * as React from "react";

import { GRID_GAP_PX, effectiveColumnWidth } from "@/lib/library/grid-window";
import { useContainerWidth } from "@/lib/use-container-width";

/**
 * WP-7.7: the effective column width of an `auto-fill, minmax(minSize,
 * 1fr)` cover grid, measured on the returned ref's element with a
 * `ResizeObserver` (`useContainerWidth`). Attach the ref to a container
 * as wide as the grid being matched (on the series page, the Related tab
 * panel spans the same column as the Issues grid), and size each cover
 * with the returned width so it matches the grid exactly and follows the
 * card-size slider live. Returns `minSize` until the first measurement.
 */
export function useGridColumnWidth<E extends HTMLElement = HTMLDivElement>(
  minSize: number,
  gap: number = GRID_GAP_PX,
): [React.RefObject<E | null>, number] {
  const [ref, width] = useContainerWidth<E>();
  return [ref, effectiveColumnWidth(width, minSize, gap)];
}
