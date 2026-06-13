import { useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { apiFetch, getCsrfToken } from "@/lib/api/auth-refresh";
import { invalidateRails } from "@/lib/api/mutations";
import { queryKeys } from "@/lib/api/queries";
import { nextPersistedProgressPage } from "@/lib/reader/webtoon-window";

const PROGRESS_DEBOUNCE_MS = 300;

/**
 * Debounced per-page progress write to `POST /progress`. Fires
 * `PROGRESS_DEBOUNCE_MS` after `currentPage` settles so a fast
 * page-flip doesn't hammer the server. Routed through `apiFetch`
 * (audit M1) so a mid-reading token refresh doesn't silently drop
 * the write.
 *
 * `finished` is sticky on the server — it's only asserted when the
 * caller lands on the last page. Mid-issue writes omit the field so
 * a jump to a bookmark can't clear a previously-finished issue.
 * Explicit "Mark as unread" goes through the mutation hook with its
 * own `finished: false`.
 *
 * Incognito short-circuits the write entirely. The reading-session
 * tracker is also gated separately by `activityTrackingEnabled` in
 * `useReadingSession`.
 *
 * Robustness (frontend-audit C10):
 * - The CSRF token is read inside the write callback, not snapshotted
 *   at mount — a token rotation mid-session (long read across a
 *   re-auth) used to 403 every subsequent write, silently, forever.
 * - A `pagehide` listener flushes the pending debounced write with
 *   `fetch(keepalive)` so the final page flip before closing the tab
 *   isn't dropped ("stopped on page 18, resumed at 17").
 *
 * Cache invalidation: after each successful write we mark the
 * shared `useUserProgress` query stale + invalidate every cached
 * rail/detail-page surface that consumes it. Without this, finishing
 * an issue in the reader and navigating back to a CBL detail page
 * (or any kebab-affording paginated list) showed the pre-read state
 * until either `useUserProgress`'s 30 s `staleTime` elapsed or the
 * route remounted from scratch. The active-observer guard is implicit
 * in TanStack — if no card is mounted (the common case while the
 * reader is open), invalidation is free: the query is just marked
 * stale and the refetch fires when the user lands back on a page
 * that subscribes.
 */
export function useReaderProgressWrite(opts: {
  issueId: string;
  currentPage: number;
  totalPages: number;
  incognito: boolean;
  /**
   * Webtoon-only (audit risk #5): persist a monotonic high-water page so
   * the scroll observer dragging `currentPage` backward (a scroll-up, or
   * the interim sweep of a programmatic jump) can't regress the saved
   * page. Off for single/double, where a PageStrip/keyboard jump-back is
   * an *intentional* resume change that should persist as-is.
   */
  monotonic?: boolean;
}): void {
  const { issueId, currentPage, totalPages, incognito, monotonic = false } =
    opts;
  const qc = useQueryClient();
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // The body the debounce timer would send, kept in a ref so the
  // pagehide flush can post it even though the timer hasn't fired.
  const pendingBody = useRef<Record<string, unknown> | null>(null);
  // High-water mark for the monotonic guard. Seeded from the first
  // (resumed) page; reset when the issue changes.
  const highWater = useRef(currentPage);
  useEffect(() => {
    highWater.current = currentPage;
    // Only reset on issue change — seeding the new issue's resume page.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [issueId]);

  useEffect(() => {
    if (!issueId) return;
    if (incognito) return;
    if (timer.current) clearTimeout(timer.current);
    const onLastPage = currentPage >= totalPages - 1;
    // `finished` stays keyed on the REAL current page — only assert it
    // when genuinely on the last page, never off the high-water mark.
    const page = monotonic
      ? nextPersistedProgressPage(highWater.current, currentPage)
      : currentPage;
    if (monotonic) highWater.current = page;
    const body: Record<string, unknown> = {
      issue_id: issueId,
      page,
    };
    if (onLastPage) body.finished = true;
    pendingBody.current = body;
    timer.current = setTimeout(() => {
      pendingBody.current = null;
      const csrf = getCsrfToken();
      void apiFetch("/progress", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(csrf ? { "X-CSRF-Token": csrf } : {}),
        },
        body: JSON.stringify(body),
      })
        .then(() => {
          // Share the invalidation set with `useUpsertIssueProgress`
          // / `useBulkMarkProgress`. This raw-apiFetch path used to
          // skip TanStack entirely, leaving rails + detail pages
          // stale after a reading session.
          qc.invalidateQueries({ queryKey: queryKeys.userProgress });
          invalidateRails(qc);
        })
        .catch(() => {
          /* best-effort; retries on next page change */
        });
    }, PROGRESS_DEBOUNCE_MS);
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, [currentPage, incognito, issueId, monotonic, qc, totalPages]);

  // Flush the in-flight debounce on tab close / app switch. keepalive
  // survives the unload and carries the CSRF header. Idempotent with
  // the timer path — the server upserts by (user, issue).
  useEffect(() => {
    if (incognito) return;
    const flush = () => {
      const body = pendingBody.current;
      if (!body) return;
      pendingBody.current = null;
      const csrf = getCsrfToken();
      void fetch("/api/progress", {
        method: "POST",
        keepalive: true,
        headers: {
          "Content-Type": "application/json",
          ...(csrf ? { "X-CSRF-Token": csrf } : {}),
        },
        body: JSON.stringify(body),
      }).catch(() => {
        /* unload race — next session's write self-heals */
      });
    };
    window.addEventListener("pagehide", flush);
    return () => window.removeEventListener("pagehide", flush);
  }, [incognito]);
}
