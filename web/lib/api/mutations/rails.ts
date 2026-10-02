/**
 * Rail-invalidation helper shared by the progress mutations and the
 * reader's raw-apiFetch progress writer. Lives in its own module so the
 * reader can import it without the whole `mutations/index.ts` barrel
 * (WP-4.4 reader bundle budget). Re-exported from `index.ts`.
 */
import type { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";

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
  qc.invalidateQueries({ queryKey: ["saved-views"], exact: false });
  qc.invalidateQueries({ queryKey: ["cbl-lists"], exact: false });
  qc.invalidateQueries({ queryKey: ["collections"], exact: false });
  qc.invalidateQueries({ queryKey: ["markers"], exact: false });
  qc.invalidateQueries({ queryKey: queryKeys.markerCount });
  // WP-7.4: similar-series rails exclude hidden + (home rail) started
  // series, so both a dismissal and a progress write can change them.
  qc.invalidateQueries({ queryKey: ["similar"], exact: false });
}
