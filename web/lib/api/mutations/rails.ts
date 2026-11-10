/**
 * Rail-invalidation helper shared by the progress mutations and the
 * reader's raw-apiFetch progress writer. Lives in its own module so the
 * reader can import it without the whole `mutations/index.ts` barrel
 * (WP-4.4 reader bundle budget). Re-exported from `index.ts`.
 */
import type { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import { queryKeyPrefixes } from "../query-key-prefixes";

/** Invalidate every cached surface that derives from per-user reading
 *  progress: the two system rails, the saved-views index + per-view
 *  results, the CBL list rail/grid variants, the collection rail/grid
 *  variants, the marker + bookmarks listing, and the bookmarks badge.
 *
 *  Used by every progress-mutating hook (upsert / bulk-mark / dismiss)
 *  AND by the reader's raw-apiFetch progress writer
 *  (`useReaderProgressWrite`) — both need the same invalidation set
 *  so navigating back from `/read/...` to a paginated detail page
 *  doesn't show stale "unread" state on a just-finished issue.
 *
 *  The previous narrower helper missed `cbl-lists/window`, `cbl-lists/
 *  entries`, `collections/entries`, and the bookmark surfaces, which
 *  caused stale cards after a kebab "Mark as read" on the home rails
 *  and `/views/[id]` detail pages. See
 *  [docs/dev/multi-select.md](docs/dev/multi-select.md) for the rail
 *  inventory that drove the broadening.
 */
export function invalidateRails(qc: ReturnType<typeof useQueryClient>) {
  qc.invalidateQueries({ queryKey: queryKeys.continueReading });
  qc.invalidateQueries({ queryKey: queryKeys.onDeck });
  qc.invalidateQueries({ queryKey: queryKeys.recentHistory });
  qc.invalidateQueries({ queryKey: queryKeys.randomPicks });
  qc.invalidateQueries({ queryKey: queryKeys.railDismissals });
  qc.invalidateQueries({ queryKey: ["saved-views"], exact: false });
  qc.invalidateQueries({ queryKey: ["cbl-lists"], exact: false });
  qc.invalidateQueries({ queryKey: ["collections"], exact: false });
  qc.invalidateQueries({ queryKey: ["markers"], exact: false });
  qc.invalidateQueries({ queryKey: queryKeys.markerCount });
  // WP-7.4: similar-series rails exclude hidden + (home rail) started
  // series, so both a dismissal and a progress write can change them.
  qc.invalidateQueries({ queryKey: ["similar"], exact: false });
}

/** Invalidate the queries that carry a series' per-user read progress
 *  (`SeriesView.progress_summary`): every series detail
 *  (`queryKeys.series(id)`, keyed by slug on the series page and by id
 *  in the reader), the series listings (`queryKeys.seriesList`) and the
 *  entity pages' series grids (`queryKeys.entityItems(…, "series", …)`).
 *  A progress write only knows issue ids, so it can't name the series;
 *  the keys are narrow enough that matching every series is cheap — only
 *  a mounted series detail refetches, and the listings are only marked
 *  stale. Saved-view, collection and similar-series listings are already
 *  covered by [`invalidateRails`].
 *
 *  Called next to `invalidateRails` by every progress-changing mutation
 *  and the reader's progress writer. The reader passes
 *  `refetchActive: false`: it writes on every page turn and only reads
 *  its series summary once per issue, so it marks the queries stale
 *  without refetching its own. */
export function invalidateSeriesProgress(
  qc: ReturnType<typeof useQueryClient>,
  opts: { refetchActive?: boolean } = {},
) {
  const refetchType = opts.refetchActive === false ? "none" : "active";
  // `["series", id]` only: deeper keys under the prefix (issues,
  // metadata, coverage, relationships, …) carry no progress.
  qc.invalidateQueries({
    queryKey: queryKeyPrefixes.series,
    predicate: (q) => q.queryKey.length === 2,
    refetchType,
  });
  // Listings are marked stale, never refetched in place: a mounted
  // infinite list would re-request every loaded page for one mark-read.
  // They pick the change up on their next mount or focus.
  qc.invalidateQueries({
    queryKey: queryKeyPrefixes.seriesList,
    refetchType: "none",
  });
  qc.invalidateQueries({
    queryKey: queryKeyPrefixes.entities,
    predicate: (q) => q.queryKey[3] === "series",
    refetchType: "none",
  });
}
