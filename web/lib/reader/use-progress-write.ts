import { useEffect, useMemo, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { apiFetch } from "@/lib/api/auth-refresh";
import { invalidateRails } from "@/lib/api/mutations";
import { queryKeys } from "@/lib/api/queries";

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
}): void {
  const { issueId, currentPage, totalPages, incognito } = opts;
  const qc = useQueryClient();
  const csrfToken = useMemo(() => {
    if (typeof document === "undefined") return "";
    const m = document.cookie.match(/(?:^|;\s*)(?:__Host-)?comic_csrf=([^;]+)/);
    return m ? decodeURIComponent(m[1]!) : "";
  }, []);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (!issueId) return;
    if (incognito) return;
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      const onLastPage = currentPage >= totalPages - 1;
      const body: Record<string, unknown> = {
        issue_id: issueId,
        page: currentPage,
      };
      if (onLastPage) body.finished = true;
      void apiFetch("/progress", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(csrfToken ? { "X-CSRF-Token": csrfToken } : {}),
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
  }, [csrfToken, currentPage, incognito, issueId, qc, totalPages]);
}
