"use client";

import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { apiFetch } from "./auth-refresh";
import { queryKeys } from "./queries";
import type { ScanEvent } from "./types";

type Status = "connecting" | "open" | "closed";

/** How long to coalesce invalidations before flushing them as one batch
 *  (audit G5). A scan or bulk-metadata apply emits a storm of events; a
 *  fixed window means at most one `invalidateQueries` sweep per window
 *  instead of one per event. Fixed (not resetting) so a long-running
 *  storm still flushes periodically rather than starving until it ends. */
const INVALIDATION_FLUSH_MS = 1500;

/**
 * Map a scan-event to the query-key prefixes it should invalidate. Pure
 * + exported so the routing is unit-testable (the table-driven test
 * enumerates every event type → expected scopes, and the exhaustive
 * switch fails the build if a new `ScanEvent` variant is added without a
 * mapping — audit risk #3).
 *
 * Library-keyed events scope to that library's caches; the broad
 * `["series"]` / `["issues"]` prefixes stay broad on purpose — the
 * series/issue caches are slug-keyed while events are id-keyed, so a
 * narrower scope would miss an open detail/issues page. `lagged` (the WS
 * dropped events) triggers a broad recovery sweep over everything the
 * socket normally drives — this is what lets the WS-redundant polls go.
 */
export function invalidationsForEvent(
  evt: ScanEvent,
): readonly (readonly unknown[])[] {
  switch (evt.type) {
    case "scan.started":
    case "scan.failed":
      return [queryKeys.scanRunsAll(evt.library_id)];
    case "scan.completed":
      return [
        queryKeys.scanRunsAll(evt.library_id),
        queryKeys.library(evt.library_id),
        queryKeys.health(evt.library_id),
        queryKeys.removed(evt.library_id),
        ["series"],
      ];
    case "scan.health_issue":
      return [queryKeys.health(evt.library_id)];
    case "scan.series_updated":
      // Previously unhandled — a per-series scan change never refreshed
      // its row. Series caches are slug-keyed, so invalidate broadly.
      return [["series"]];
    case "thumbs.started":
    case "thumbs.completed":
    case "thumbs.failed":
      return [queryKeys.thumbnailsStatus(evt.library_id), queryKeys.queueDepth];
    case "metadata.applied":
      // DB-direct apply: refresh issue/series caches + the admin metadata
      // dashboards (whose 60s poll is dropped now this covers them).
      return [
        ["issues"],
        ["series"],
        queryKeys.adminMetadataDashboard,
        queryKeys.adminMetadataMatchQuality,
        // Prefix-match every limit of the recent-applies summary (B14).
        ["admin", "metadata", "recent-applies"],
      ];
    case "backfill.completed":
      // A background backfill drain finished — refresh the queue depth pill
      // and the metadata dashboard (which hosts the backfill cards). B17.
      return [queryKeys.queueDepth, queryKeys.adminMetadataDashboard];
    case "lagged":
      // We missed events — recover by sweeping every WS-driven cache.
      return [["libraries"], ["admin"], ["series"], ["issues"]];
    case "scan.progress":
      // Live progress is consumed from the events buffer, not the cache.
      return [];
    default:
      return assertNever(evt);
  }
}

function assertNever(evt: never): readonly (readonly unknown[])[] {
  // A new ScanEvent variant reached here without a mapping. Don't throw
  // (a stray event shouldn't crash the reader); the `never` type makes
  // it a compile error, which is the real guard.
  void evt;
  return [];
}

/**
 * Module-level dedupe so multiple `useScanEvents` instances in the same tab
 * (admin shell + library overview + a per-library detail page) don't each
 * fire their own toast for the same `scan_id`. Bounded so a long-lived tab
 * with many scans doesn't grow the set forever.
 */
const TOASTED_SCANS = new Set<string>();
const TOASTED_SCANS_CAP = 200;
function rememberScanToast(scanId: string): boolean {
  if (TOASTED_SCANS.has(scanId)) return false;
  if (TOASTED_SCANS.size >= TOASTED_SCANS_CAP) {
    // Drop the oldest insertion (Set preserves insertion order).
    const first = TOASTED_SCANS.values().next().value;
    if (first !== undefined) TOASTED_SCANS.delete(first);
  }
  TOASTED_SCANS.add(scanId);
  return true;
}

function getCsrfToken(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)__Host-comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}

/**
 * Mint a one-time WebSocket auth ticket via `POST /api/auth/ws-ticket`.
 * Uses `apiFetch` so a stale access cookie auto-refreshes once; the WS
 * handshake itself then carries the ticket as a query param and doesn't
 * need cookies (which lets dev work cross-origin between :3000 and :8080).
 */
async function fetchWsTicket(): Promise<string | null> {
  const csrf = getCsrfToken();
  try {
    const res = await apiFetch("/auth/ws-ticket", {
      method: "POST",
      headers: {
        Accept: "application/json",
        ...(csrf ? { "X-CSRF-Token": csrf } : {}),
      },
    });
    if (!res.ok) return null;
    const body = (await res.json()) as { ticket?: string };
    return body.ticket ?? null;
  } catch {
    return null;
  }
}

/**
 * Subscribe to `/ws/scan-events` and route events into TanStack Query
 * invalidations + an in-memory ring buffer. Auto-reconnects with exponential
 * backoff (capped at 30s). On every (re)connect we mint a fresh ticket via
 * §9.6's auth path, so the WS upgrade can authenticate cross-origin in dev
 * and same-origin in prod.
 *
 * The backend filters to admin-only, so we don't add a per-user
 * library-access check here.
 */
export function useScanEvents(opts?: {
  /** When set, only events for this library land in `events`. */
  libraryId?: string;
  /** Cap on the in-memory ring buffer (default 200). */
  maxBuffer?: number;
  /** Surface scan.failed and scan.health_issue (severity=error) as toasts. */
  toastErrors?: boolean;
  /**
   * Surface scan.completed as a success toast. Default true so any admin
   * who triggers a scan (library / series / issue) gets a confirmation
   * when it finishes; toast is deduped by `scan_id` across subscribers.
   */
  toastCompletions?: boolean;
}) {
  const {
    libraryId,
    maxBuffer = 200,
    toastErrors = true,
    toastCompletions = true,
  } = opts ?? {};
  const qc = useQueryClient();
  const [status, setStatus] = useState<Status>("connecting");
  const [events, setEvents] = useState<ScanEvent[]>([]);

  useEffect(() => {
    let socket: WebSocket | null = null;
    let cancelled = false;
    let attempt = 0;
    let timer: ReturnType<typeof setTimeout> | null = null;

    // ── Coalesced invalidation (audit G5) ──
    // Collect query-key prefixes from the event stream and flush them as
    // one deduped batch per `INVALIDATION_FLUSH_MS` window, instead of
    // firing `invalidateQueries` per event (a scan / bulk apply storms
    // them). `pending` maps a JSON-stringified key → the key itself, for
    // dedup.
    const pending = new Map<string, readonly unknown[]>();
    let flushTimer: ReturnType<typeof setTimeout> | null = null;
    const flushInvalidations = () => {
      flushTimer = null;
      if (pending.size === 0) return;
      const keys = [...pending.values()];
      pending.clear();
      for (const key of keys) {
        qc.invalidateQueries({ queryKey: key });
      }
    };
    const enqueueInvalidations = (keys: readonly (readonly unknown[])[]) => {
      for (const key of keys) pending.set(JSON.stringify(key), key);
      // Fixed window: schedule once; don't reset, so a long storm still
      // flushes every window rather than starving until it stops.
      if (flushTimer === null && pending.size > 0) {
        flushTimer = setTimeout(flushInvalidations, INVALIDATION_FLUSH_MS);
      }
    };

    if (typeof window === "undefined") return;
    // In prod the Rust binary serves both the HTML and the WS endpoint at
    // the same origin. In dev the page is on Next dev (:3000); Next's
    // rewrite layer doesn't proxy WS upgrades, so we connect directly to
    // the API host (default `:8080`, overridable via NEXT_PUBLIC_API_URL).
    // Either way we authenticate the upgrade with a one-time ticket.
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const isDev = process.env.NODE_ENV !== "production";
    const host = isDev
      ? new URL(process.env.NEXT_PUBLIC_API_URL ?? "http://localhost:8080").host
      : window.location.host;
    const baseUrl = `${proto}//${host}/ws/scan-events`;

    const scheduleReconnect = () => {
      if (cancelled) return;
      const delay = Math.min(30_000, 500 * 2 ** Math.min(attempt, 6));
      attempt += 1;
      timer = setTimeout(connect, delay);
    };

    const connect = async () => {
      if (cancelled) return;
      setStatus("connecting");
      const ticket = await fetchWsTicket();
      if (cancelled) return;
      if (!ticket) {
        // Likely unauthenticated or admin-only failure; back off and try
        // again so a sign-in mid-session recovers.
        setStatus("closed");
        scheduleReconnect();
        return;
      }
      const url = `${baseUrl}?ticket=${encodeURIComponent(ticket)}`;
      socket = new WebSocket(url);
      socket.addEventListener("open", () => {
        attempt = 0;
        setStatus("open");
      });
      socket.addEventListener("close", () => {
        if (cancelled) return;
        setStatus("closed");
        scheduleReconnect();
      });
      socket.addEventListener("error", () => {
        socket?.close();
      });
      socket.addEventListener("message", (ev) => {
        let evt: ScanEvent;
        try {
          evt = JSON.parse(
            typeof ev.data === "string" ? ev.data : "",
          ) as ScanEvent;
        } catch {
          return;
        }
        // Filter by library if requested. `lagged` has no library payload.
        if (libraryId && "library_id" in evt && evt.library_id !== libraryId) {
          return;
        }
        setEvents((buf) => {
          const next = [...buf, evt];
          return next.length > maxBuffer ? next.slice(-maxBuffer) : next;
        });
        // Cache invalidation: coalesced (see enqueueInvalidations). Toasts
        // stay immediate — a 1.5s-deferred "scan failed" would feel broken.
        enqueueInvalidations(invalidationsForEvent(evt));
        switch (evt.type) {
          case "scan.completed":
            if (toastCompletions && rememberScanToast(evt.scan_id)) {
              // A zero-change result is an app-state notice, not a success
              // celebration (audit UX-14): for a brand-new library it's the
              // only signal that the chosen folder had nothing to ingest.
              if (evt.added === 0 && evt.updated === 0 && evt.removed === 0) {
                toast.info(formatCompletionMessage(evt));
              } else {
                toast.success(formatCompletionMessage(evt));
              }
            }
            break;
          case "scan.failed":
            if (toastErrors && rememberScanToast(evt.scan_id)) {
              toast.error(`Scan failed: ${evt.error}`);
            }
            break;
          case "scan.health_issue":
            if (toastErrors && evt.severity === "error") {
              toast.error(
                `Health issue: ${evt.kind}${evt.path ? ` — ${evt.path}` : ""}`,
              );
            }
            break;
          case "thumbs.failed":
            if (toastErrors) {
              toast.error(`Thumbnail job failed: ${evt.error}`);
            }
            break;
          case "backfill.completed":
            // No event id; dedup the same event delivered to multiple
            // subscribers by a synthetic key (B17).
            if (
              toastCompletions &&
              rememberScanToast(
                `backfill:${evt.kind}:${evt.processed}:${evt.skipped}`,
              )
            ) {
              toast.success(formatBackfillMessage(evt));
            }
            break;
          default:
            break;
        }
      });
    };
    void connect();

    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      // Flush any pending invalidations so a queued refresh isn't lost
      // when the subscriber unmounts mid-window.
      if (flushTimer) clearTimeout(flushTimer);
      flushInvalidations();
      socket?.close();
    };
  }, [libraryId, maxBuffer, qc, toastErrors, toastCompletions]);

  return { status, events };
}

/**
 * Build the success-toast copy from a `scan.completed` event. We prefer
 * concrete numbers ("added 3, updated 5") over a generic "Scan complete"
 * because the same toast also fires for narrow per-series and per-issue
 * scans — the counts make it obvious which kind just finished.
 */
function formatCompletionMessage(
  evt: Extract<ScanEvent, { type: "scan.completed" }>,
): string {
  const parts: string[] = [];
  if (evt.added > 0) parts.push(`added ${evt.added}`);
  if (evt.updated > 0) parts.push(`updated ${evt.updated}`);
  if (evt.removed > 0) parts.push(`removed ${evt.removed}`);
  if (parts.length === 0) {
    // Doubles as the empty-folder signal for a first scan (audit UX-14):
    // the event doesn't say whether the library was already populated, so
    // the copy has to read correctly for both a no-op re-scan and a
    // pointed-at-the-wrong-folder first scan.
    return "Scan complete — no new or changed comics found.";
  }
  return `Scan complete · ${parts.join(", ")}`;
}

/** Success-toast copy for a finished backfill drain (audit B17). */
function formatBackfillMessage(
  evt: Extract<ScanEvent, { type: "backfill.completed" }>,
): string {
  const noun = evt.kind === "cover_phash" ? "cover hash" : "variant cover";
  const verb = evt.kind === "cover_phash" ? "Backfilled" : "Re-downloaded";
  if (evt.processed === 0 && evt.skipped === 0) {
    return `${noun === "cover hash" ? "Cover-hash" : "Variant-cover"} backfill complete — nothing to do.`;
  }
  let msg = `${verb} ${evt.processed.toLocaleString()} ${noun}${evt.processed === 1 ? "" : "s"}`;
  if (evt.skipped > 0) {
    msg +=
      evt.kind === "cover_phash"
        ? ` · ${evt.skipped} could not be decoded`
        : ` · ${evt.skipped} could not be fetched`;
  }
  return msg;
}
