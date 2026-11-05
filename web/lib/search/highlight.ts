/**
 * Client-side match highlighting for search-result titles. The backend
 * only returns `<mark>` snippets for summary matches; titles come back
 * plain, so the palette marks the typed text itself.
 *
 * Every whitespace-separated query term is matched case-insensitively,
 * anywhere in the text (so "spider man" marks both halves of
 * "Spider-Man"); overlapping or touching ranges merge. Pure, so vitest
 * covers it without React.
 */

export interface HighlightSegment {
  text: string;
  match: boolean;
}

/** Terms shorter than this are ignored unless they are the whole
 *  query, so "x men" doesn't light up every lone "x". */
const MIN_TERM_LEN = 2;

export function highlightSegments(
  text: string,
  query: string,
): HighlightSegment[] {
  if (!text) return [];
  const q = query.trim();
  if (!q) return [{ text, match: false }];
  const lower = text.toLowerCase();
  // `toLowerCase` can change length for a few code points (e.g. "İ");
  // indices would drift, so skip highlighting rather than mis-mark.
  if (lower.length !== text.length) return [{ text, match: false }];

  const terms = Array.from(
    new Set(
      q
        .toLowerCase()
        .split(/\s+/)
        .filter((t) => t.length >= MIN_TERM_LEN || t === q.toLowerCase()),
    ),
  );
  const ranges: Array<[number, number]> = [];
  for (const term of terms) {
    if (!term) continue;
    let from = 0;
    for (;;) {
      const at = lower.indexOf(term, from);
      if (at < 0) break;
      ranges.push([at, at + term.length]);
      from = at + term.length;
    }
  }
  if (ranges.length === 0) return [{ text, match: false }];

  ranges.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: Array<[number, number]> = [];
  for (const r of ranges) {
    const last = merged[merged.length - 1];
    if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1]);
    else merged.push([r[0], r[1]]);
  }

  const out: HighlightSegment[] = [];
  let cursor = 0;
  for (const [start, end] of merged) {
    if (start > cursor)
      out.push({ text: text.slice(cursor, start), match: false });
    out.push({ text: text.slice(start, end), match: true });
    cursor = end;
  }
  if (cursor < text.length)
    out.push({ text: text.slice(cursor), match: false });
  return out;
}
