/**
 * Card-size bounds for the series page's View → Card size slider
 * (IssuesPanel). The issue grid uses `repeat(auto-fill, minmax(<size>px,
 * 1fr))` so column count adapts fluidly as the user drags. Step matches a
 * comic cover's natural aspect ratio increments — finer steps just look
 * like jitter.
 *
 * WP-7.7: shared with the Related tab (similar-series rail, reading-order
 * strip, relationship cards, same-universe section), which reads the same
 * storage key through `useCardSize` so every cover on the page follows
 * the one slider.
 */
export const SERIES_CARD_SIZE = {
  storageKey: "folio.series.cardSize",
  min: 120,
  max: 280,
  step: 20,
  defaultSize: 160,
} as const;
