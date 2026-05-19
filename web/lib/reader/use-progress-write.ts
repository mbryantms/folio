import { useEffect, useMemo, useRef } from "react";
import { apiFetch } from "@/lib/api/auth-refresh";

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
 */
export function useReaderProgressWrite(opts: {
  issueId: string;
  currentPage: number;
  totalPages: number;
  incognito: boolean;
}): void {
  const { issueId, currentPage, totalPages, incognito } = opts;
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
      }).catch(() => {
        /* best-effort; retries on next page change */
      });
    }, PROGRESS_DEBOUNCE_MS);
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, [csrfToken, currentPage, incognito, issueId, totalPages]);
}
