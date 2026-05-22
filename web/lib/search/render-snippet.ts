/**
 * Strict HTML sanitiser for `ts_headline()` snippets returned by the
 * search endpoints. The backend constrains output to `<mark>…</mark>`
 * around matched terms, but the surrounding excerpt is user-supplied
 * `summary` text — which might contain stray angle brackets or HTML
 * entities. We can't trust the whole string for raw HTML insertion
 * even though the markup is "ours".
 *
 * Approach: tokenise into alternating (text, `<mark>` / `</mark>`)
 * chunks via a single regex pass. Text chunks pass through
 * `escapeHtml`; tag chunks (already known safe — the regex only
 * matches the literal allowed forms) re-emit as canonical lowercase
 * markup. The result is safe to render via `dangerouslySetInnerHTML`.
 *
 * Used by `SearchModal.tsx` and `SearchView.tsx`. Pure function so
 * vitest can exercise it without React.
 */

const TAG_PATTERN = /<\/?mark>/gi;

export function renderSearchSnippet(raw: string): string {
  if (!raw) return "";
  let out = "";
  let cursor = 0;
  for (const match of raw.matchAll(TAG_PATTERN)) {
    const start = match.index ?? 0;
    if (start > cursor) {
      out += escapeHtml(raw.slice(cursor, start));
    }
    out += match[0].toLowerCase();
    cursor = start + match[0].length;
  }
  if (cursor < raw.length) {
    out += escapeHtml(raw.slice(cursor));
  }
  return out;
}

function escapeHtml(s: string): string {
  return s
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}
