/**
 * Shared sizing for the per-issue square grids so the series page's
 * **Collection** tab (ownership + metadata status) and **Activity** tab
 * (read-count heatmap) line up cell-for-cell. Each grid keeps its own cell
 * coloring/content — only the layout (cell size, gap, corner radius) is shared.
 */

/** Grid container: auto-fill columns of equal-width square cells. */
export const ISSUE_GRID_COLS =
  "grid grid-cols-[repeat(auto-fill,minmax(2.25rem,1fr))] gap-1.5";

/** Corner radius applied to every grid cell (square comes from `aspect-square`
 *  on the cell itself). */
export const ISSUE_GRID_CELL_RADIUS = "rounded-md";
