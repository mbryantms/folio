/**
 * Issue-category search filters for the `/search` page (audit B-series,
 * "read-status scoping on issue results").
 *
 * The issue results grid is backed by the paginated `/issues` cross-list
 * (`useIssuesCrossListInfinite`), so unlike the relevance-capped
 * `/issues/search` it can carry a server-applied `read_status` facet
 * without truncating. Today the only issue-grid facet is read status; the
 * shape is kept as a list so adding more (year, publisher, …) later mirrors
 * the series filter module.
 */

/** The three valid read-status tokens + their labels. Mirrors the
 *  library grid's `READ_STATUS_OPTIONS` (kept local rather than imported
 *  so this module — used by the server `page.tsx` — doesn't pull the
 *  client `FilterSheet` component into the server bundle). */
export const ISSUE_READ_STATUS_OPTIONS: { value: string; label: string }[] = [
  { value: "unread", label: "Unread" },
  { value: "in_progress", label: "Reading" },
  { value: "read", label: "Read" },
];

const VALID = new Set(ISSUE_READ_STATUS_OPTIONS.map((o) => o.value));

/** Parse `?read_status=unread,read` into a de-duplicated, validated list.
 *  Unknown tokens are dropped so a hand-edited URL can't 422 the grid. */
export function parseIssueReadStatus(
  params: Record<string, string | undefined>,
): string[] {
  const raw = params.read_status;
  if (!raw) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const tok of raw.split(",").map((s) => s.trim())) {
    if (VALID.has(tok) && !seen.has(tok)) {
      seen.add(tok);
      out.push(tok);
    }
  }
  return out;
}

/** Serialize the selected statuses to a `read_status` CSV. Returns
 *  `undefined` when nothing (or everything) is selected — both are a
 *  server-side no-op, so the param is dropped to keep URLs clean. */
export function issueReadStatusToParam(values: string[]): string | undefined {
  const valid = values.filter((v) => VALID.has(v));
  if (valid.length === 0 || valid.length === VALID.size) return undefined;
  return valid.join(",");
}
