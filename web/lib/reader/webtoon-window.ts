/**
 * Pure layout/progress math for the windowed webtoon reader (audit
 * C1b/risk #5). Kept side-effect-free so it's unit-testable in the
 * node-env harness; the DOM/observer wiring lives in `Reader.tsx`.
 */

/** Pages mounted on each side of the current page. 11 mounted max — well
 *  past a viewport's worth plus scroll runway, small enough that a
 *  200-page chapter no longer mounts 200 images + MarkerOverlays. */
export const WEBTOON_WINDOW_RADIUS = 5;

/**
 * Inclusive `[start, end]` range of page indices whose heavy content
 * (image + marker overlay) should mount, centered on `currentPage` and
 * clamped to the issue. Off-range slots render as sized placeholders.
 * `totalPages <= 0` yields an empty range (`end < start`).
 */
export function computeWebtoonWindow(
  currentPage: number,
  totalPages: number,
  radius: number = WEBTOON_WINDOW_RADIUS,
): { start: number; end: number } {
  if (totalPages <= 0) return { start: 0, end: -1 };
  const c = Math.max(0, Math.min(totalPages - 1, currentPage));
  return {
    start: Math.max(0, c - radius),
    end: Math.min(totalPages - 1, c + radius),
  };
}

/**
 * CSS `aspect-ratio` string for a page's reserved placeholder height.
 * Uses the server-known dims when present so the placeholder is the
 * exact height the decoded image will be (no scroll-jump, correct
 * resume); falls back to a 2:3 comic page when dims are missing.
 */
export function placeholderAspectRatio(page?: {
  image_width?: number | null;
  image_height?: number | null;
}): string {
  const w = page?.image_width;
  const h = page?.image_height;
  return w && h && w > 0 && h > 0 ? `${w} / ${h}` : "2 / 3";
}

/**
 * Risk #5 guard: persisted progress is monotonic within a session, so
 * the webtoon observer dragging `currentPage` *backward* during a scroll
 * (or a programmatic jump's interim sweep) can't regress the saved page.
 * The on-screen page is free to move both ways; only what we persist is
 * clamped to the high-water mark.
 */
export function nextPersistedProgressPage(
  prev: number,
  current: number,
): number {
  return Math.max(prev, current);
}
