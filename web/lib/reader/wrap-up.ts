/**
 * End-of-issue "Wrap up" — the data the end-of-issue card (and the
 * webtoon end footer) need to let the reader rate the issue they just
 * finished, leave an issue-level closing note, and review the markers
 * they made in it. Pure types + helpers; the reader wires the mutations
 * and hands the card plain values + callbacks so the card stays
 * renderable to static markup in tests.
 */
import type { MarkerView } from "@/lib/api/types";

export type MarkerCounts = {
  total: number;
  bookmarks: number;
  notes: number;
  favorites: number;
  highlights: number;
};

export type WrapUpProps = {
  /** "Saga #11" — the issue the reader just finished. */
  issueLabel: string;
  /** Current user rating, 0..=5 half-steps, or `null`. */
  rating: number | null;
  onRate: (next: number | null) => void;
  /** Issue-level closing note (not a page note). */
  note: string | null;
  onSaveNote: (body: string | null) => void;
  noteSaving?: boolean;
  markerCounts: MarkerCounts;
  /** Opens the in-reader marker drawer. */
  onReviewMarkers: () => void;
};

export const EMPTY_MARKER_COUNTS: MarkerCounts = {
  total: 0,
  bookmarks: 0,
  notes: 0,
  favorites: 0,
  highlights: 0,
};

/** Per-kind tally of the markers on an issue, from the same
 *  `GET /me/issues/{id}/markers` payload the overlay renders. */
export function summarizeMarkers(
  items: ReadonlyArray<Pick<MarkerView, "kind">> | undefined,
): MarkerCounts {
  const c = { ...EMPTY_MARKER_COUNTS };
  for (const m of items ?? []) {
    c.total += 1;
    if (m.kind === "bookmark") c.bookmarks += 1;
    else if (m.kind === "note") c.notes += 1;
    else if (m.kind === "favorite") c.favorites += 1;
    else if (m.kind === "highlight") c.highlights += 1;
  }
  return c;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "2 highlights, 1 note" — the non-zero kinds, highlights first.
 *  Empty string when there is nothing to break down. */
export function markerBreakdown(c: MarkerCounts): string {
  const parts: string[] = [];
  if (c.highlights) parts.push(plural(c.highlights, "highlight", "highlights"));
  if (c.notes) parts.push(plural(c.notes, "note", "notes"));
  if (c.bookmarks) parts.push(plural(c.bookmarks, "bookmark", "bookmarks"));
  if (c.favorites) parts.push(plural(c.favorites, "favorite", "favorites"));
  return parts.join(", ");
}

/** "3 markers" / "1 marker" / "No markers". */
export function markerTotalLabel(c: MarkerCounts): string {
  if (c.total === 0) return "No markers";
  return plural(c.total, "marker", "markers");
}

/** Rating keys while the end card is open: `1`–`5` set whole stars.
 *  `0` is left to the reader's zoom-reset binding; clearing is a second
 *  press on the current value, same as clicking the selected star. */
export function ratingFromKey(
  key: string,
  current: number | null,
): number | null | undefined {
  if (!/^[1-5]$/.test(key)) return undefined;
  const n = Number(key);
  return current !== null && Math.abs(current - n) < 0.001 ? null : n;
}
