"use client";

/**
 * M6a reading-session tracker.
 *
 * Captures intentional reading (not browsing) by listening to the same
 * `currentPage` changes the existing progress writer uses, accumulating
 * `active_ms` while the tab is visible / focused / non-idle, and posting a
 * heartbeat upsert every 30s. The same `client_session_id` is used for the
 * initial write, every heartbeat, and the final flush — the server's
 * `(user_id, client_session_id)` unique index keeps it idempotent.
 *
 * Design notes
 * - One source, two sinks: the same callback writes the per-page progress
 *   (already wired in `Reader.tsx`) and updates the in-memory session
 *   aggregator. We do NOT remove the existing progress writer — sessions
 *   are coarse (30s heartbeat) while progress is fine-grained (300ms
 *   debounce per page turn) and they answer different questions.
 * - sendBeacon final flush: CSRF middleware doesn't support custom headers
 *   on sendBeacon, so the final close is best-effort. The server's
 *   dangling-session sweeper closes any session whose `last_heartbeat_at`
 *   is > 5 min stale.
 * - Strict mode: we generate `client_session_id` once per mount and store
 *   it in a ref. The unique constraint on the server makes any
 *   double-submit during dev-mode double-mount idempotent.
 * - Webtoon mode has no discrete page turns; the caller passes the
 *   currently-visible page index from the reader's scroll sampler.
 */

import { useCallback, useEffect, useRef } from "react";
import { usePathname } from "next/navigation";

const HEARTBEAT_INTERVAL_MS = 30_000;
const TICK_INTERVAL_MS = 1_000;
const MIN_DWELL_MS_PER_PAGE = 1_500;

export type SessionTrackerOptions = {
  /** Stable issue id from the route. */
  issueId: string;
  /** Total number of pages in the issue (server-known). */
  totalPages: number;
  /** Currently-visible page index (0-based). */
  currentPage: number;
  /** `'single' | 'double' | 'webtoon'` */
  viewMode: "single" | "double" | "webtoon";
  /** When the user has multiple pages visible (double-page mode), pass
   *  both indices so they each count toward `distinct_pages_read`. */
  visiblePages?: ReadonlyArray<number>;
  /** From `useMe()` — short-circuits the entire tracker when false. */
  trackingEnabled: boolean;
  /** Server-enforced minima (also used client-side as the gate before
   *  posting). Defaults match the migration but the caller should pass
   *  the user's actual prefs from `MeView`. */
  minActiveMs?: number;
  minPages?: number;
  /** Idle threshold in ms — input gap that ends the session. */
  idleMs?: number;
  /** Optional device tag so a future "active devices" view can attribute
   *  sessions. The reader currently passes a literal string. */
  device?: string | null;
};

/**
 * Wires up the session tracker. Returns nothing visible — it owns its
 * timers internally and tears down on unmount. Safe to call inside an
 * effect-heavy component; the hook is a no-op when `trackingEnabled` is
 * false.
 */
export function useReadingSession(opts: SessionTrackerOptions): void {
  const {
    issueId,
    totalPages,
    currentPage,
    viewMode,
    visiblePages,
    trackingEnabled,
    minActiveMs = 30_000,
    minPages = 3,
    idleMs = 180_000,
    device = "web",
  } = opts;

  // Mutable state lives in refs so the heartbeat / tick timers don't
  // re-register on every render. The timing refs are nullable and
  // lazy-initialised on the first effect run — calling `performance.now()`
  // or `new Date()` during render would violate React's purity rules and
  // also produce unstable values on re-render.
  const clientSessionIdRef = useRef<string | null>(null);
  const startedAtRef = useRef<Date | null>(null);
  const activeMsRef = useRef<number>(0);
  const pageTurnsRef = useRef<number>(0);
  const distinctPagesRef = useRef<Set<number>>(new Set());
  const startPageRef = useRef<number>(currentPage);
  const endPageRef = useRef<number>(currentPage);
  const lastInputAtRef = useRef<number | null>(null);
  const lastPageDwellStartRef = useRef<number | null>(null);
  const lastVisitedPageRef = useRef<number>(currentPage);
  const flushedRef = useRef<boolean>(false);
  const startedRef = useRef<boolean>(false);

  const pathname = usePathname();

  // First-render init for the timing/identity refs. Only fires once per
  // mount; the unique server-side `(user_id, client_session_id)` index
  // makes any strict-mode double-mount commit idempotent.
  useEffect(() => {
    if (clientSessionIdRef.current === null) {
      clientSessionIdRef.current = generateSessionId();
    }
    if (startedAtRef.current === null) {
      startedAtRef.current = new Date();
    }
    const now = performance.now();
    if (lastInputAtRef.current === null) lastInputAtRef.current = now;
    if (lastPageDwellStartRef.current === null)
      lastPageDwellStartRef.current = now;
  }, []);

  // Keep a noteworthy-page-set update on every page change.
  useEffect(() => {
    if (!trackingEnabled) return;
    const now = performance.now();
    // Track input (page turn = activity).
    lastInputAtRef.current = now;

    // If the user dwelled long enough on the previous page, count it.
    const prev = lastVisitedPageRef.current;
    const dwellStart = lastPageDwellStartRef.current ?? now;
    if (now - dwellStart >= MIN_DWELL_MS_PER_PAGE) {
      distinctPagesRef.current.add(prev);
    }

    // Update envelope + counters for the new page (and double-page partner).
    if (currentPage !== prev) {
      pageTurnsRef.current += 1;
      startedRef.current = true; // first turn = real session start
    }
    if (currentPage < startPageRef.current) startPageRef.current = currentPage;
    if (currentPage > endPageRef.current) endPageRef.current = currentPage;
    if (visiblePages) {
      for (const p of visiblePages) {
        if (p < startPageRef.current) startPageRef.current = p;
        if (p > endPageRef.current) endPageRef.current = p;
      }
    }

    lastVisitedPageRef.current = currentPage;
    lastPageDwellStartRef.current = now;
  }, [currentPage, trackingEnabled, visiblePages]);

  // Track input events that don't change `currentPage` (scroll, key,
  // pointer) so the active-time accumulator survives long dwells.
  useEffect(() => {
    if (!trackingEnabled) return;
    const onActivity = () => {
      lastInputAtRef.current = performance.now();
    };
    window.addEventListener("keydown", onActivity, { passive: true });
    window.addEventListener("pointerdown", onActivity, { passive: true });
    window.addEventListener("scroll", onActivity, { passive: true });
    window.addEventListener("wheel", onActivity, { passive: true });
    return () => {
      window.removeEventListener("keydown", onActivity);
      window.removeEventListener("pointerdown", onActivity);
      window.removeEventListener("scroll", onActivity);
      window.removeEventListener("wheel", onActivity);
    };
  }, [trackingEnabled]);

  // 1Hz tick: accumulate active_ms only while visible+focused+non-idle.
  useEffect(() => {
    if (!trackingEnabled) return;
    let lastTickAt = performance.now();
    const id = window.setInterval(() => {
      const now = performance.now();
      const elapsed = now - lastTickAt;
      lastTickAt = now;
      const visible = document.visibilityState === "visible";
      const focused = document.hasFocus();
      const lastInput = lastInputAtRef.current ?? now;
      const idle = now - lastInput > idleMs;
      if (visible && focused && !idle) {
        activeMsRef.current += elapsed;
      }
    }, TICK_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, [trackingEnabled, idleMs]);

  // Build the upsert payload from the current refs. Returns null if the
  // session hasn't been initialised yet (init effect not run).
  const buildPayload = useCallback(
    (final: boolean) => {
      const sid = clientSessionIdRef.current;
      const startedAt = startedAtRef.current;
      if (!sid || !startedAt) return null;
      // Lock in the current page if it dwelled long enough — otherwise the
      // last page never gets credit.
      const now = performance.now();
      const dwellStart = lastPageDwellStartRef.current ?? now;
      if (now - dwellStart >= MIN_DWELL_MS_PER_PAGE) {
        distinctPagesRef.current.add(lastVisitedPageRef.current);
      }
      const distinctPagesRead = distinctPagesRef.current.size;
      const startPage = clamp(
        startPageRef.current,
        0,
        Math.max(0, totalPages - 1),
      );
      const endPage = clamp(
        Math.max(endPageRef.current, startPage),
        0,
        Math.max(0, totalPages - 1),
      );
      return {
        client_session_id: sid,
        issue_id: issueId,
        started_at: startedAt.toISOString(),
        ended_at: final ? new Date().toISOString() : undefined,
        active_ms: Math.floor(activeMsRef.current),
        distinct_pages_read: distinctPagesRead,
        page_turns: pageTurnsRef.current,
        start_page: startPage,
        end_page: endPage,
        device: device ?? null,
        view_mode: viewMode,
      };
    },
    [device, issueId, totalPages, viewMode],
  );

  // 30s heartbeat — only after the user has actually started reading.
  useEffect(() => {
    if (!trackingEnabled) return;
    const id = window.setInterval(() => {
      if (!startedRef.current) return;
      const payload = buildPayload(false);
      if (payload) void postSession(payload);
    }, HEARTBEAT_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, [trackingEnabled, buildPayload]);

  // Final flush triggers — pagehide, visibilitychange→hidden,
  // route change away from /read/{id}, unmount.
  const finalize = useCallback(() => {
    if (flushedRef.current) return;
    if (!startedRef.current) return;
    const active = Math.floor(activeMsRef.current);
    const distinct = distinctPagesRef.current.size;
    if (active < minActiveMs && distinct < minPages) {
      // Below threshold — nothing worth posting.
      flushedRef.current = true;
      return;
    }
    flushedRef.current = true;
    const payload = buildPayload(true);
    if (!payload) return;
    // Best-effort: try sendBeacon (CSRF will reject; that's expected — the
    // dangling-session sweeper finishes the row server-side). We still
    // attempt the regular fetch which has CSRF — typically that succeeds
    // before the tab dies on desktop.
    void postSession(payload);
    if (typeof navigator !== "undefined" && navigator.sendBeacon) {
      const blob = new Blob([JSON.stringify(payload)], {
        type: "application/json",
      });
      navigator.sendBeacon("/me/reading-sessions", blob);
    }
  }, [buildPayload, minActiveMs, minPages]);

  useEffect(() => {
    if (!trackingEnabled) return;
    const onPageHide = () => finalize();
    const onVisibilityChange = () => {
      if (document.visibilityState === "hidden") finalize();
    };
    window.addEventListener("pagehide", onPageHide);
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      window.removeEventListener("pagehide", onPageHide);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [trackingEnabled, finalize]);

  // Pathname change away from the reader (Next.js client nav) → finalize.
  // useReadingSession is mounted at /read/{id}; if pathname diverges from
  // that prefix, the user has left.
  const initialPathRef = useRef<string | null>(null);
  useEffect(() => {
    if (initialPathRef.current === null) {
      initialPathRef.current = pathname;
      return;
    }
    if (pathname !== initialPathRef.current) {
      finalize();
    }
  }, [pathname, finalize]);

  // Unmount → final attempt (covers hot reload, error boundary tear-down).
  useEffect(() => {
    return () => {
      finalize();
    };
  }, [finalize]);
}

// ────────────── Helpers ──────────────

function generateSessionId(): string {
  // Use crypto.randomUUID() where available (HTTPS or localhost). Fall back
  // to a 64-char hex string built from getRandomValues — still well under
  // the server's 64-char cap.
  const c: Crypto | undefined =
    typeof crypto === "undefined" ? undefined : crypto;
  if (c && typeof c.randomUUID === "function") {
    return c.randomUUID();
  }
  if (c && typeof c.getRandomValues === "function") {
    const arr = new Uint8Array(16);
    c.getRandomValues(arr);
    return Array.from(arr)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
  }
  // Last-resort fallback (non-cryptographic). Sessions are user-scoped so
  // the consequence of a collision is one merged row, not a security
  // boundary breach.
  return `s${Date.now()}${Math.random().toString(36).slice(2, 14)}`;
}

function clamp(n: number, lo: number, hi: number): number {
  if (n < lo) return lo;
  if (n > hi) return hi;
  return n;
}

function getCsrfToken(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)__Host-comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}

async function postSession(body: unknown): Promise<void> {
  try {
    const csrf = getCsrfToken();
    await fetch("/api/me/reading-sessions", {
      method: "POST",
      credentials: "include",
      headers: {
        "Content-Type": "application/json",
        ...(csrf ? { "X-CSRF-Token": csrf } : {}),
      },
      body: JSON.stringify(body),
    });
  } catch {
    /* best-effort; the next heartbeat will retry the same row */
  }
}
