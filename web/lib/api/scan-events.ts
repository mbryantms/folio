"use client";

import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { apiFetch } from "./auth-refresh";
import { queryKeys } from "./queries";
import type { ScanEvent } from "./types";

type Status = "connecting" | "open" | "closed";

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
        // Cache invalidation routing.
        switch (evt.type) {
          case "scan.completed":
            qc.invalidateQueries({
              queryKey: queryKeys.scanRunsAll(evt.library_id),
            });
            qc.invalidateQueries({
              queryKey: queryKeys.library(evt.library_id),
            });
            qc.invalidateQueries({
              queryKey: queryKeys.health(evt.library_id),
            });
            qc.invalidateQueries({
              queryKey: queryKeys.removed(evt.library_id),
            });
            // Also nudge series / issue listings — a per-series or per-issue
            // scan changes the row underneath an open page, and `router.refresh()`
            // alone won't pick up timestamps in TanStack Query caches.
            qc.invalidateQueries({ queryKey: ["series"], exact: false });
            if (toastCompletions && rememberScanToast(evt.scan_id)) {
              toast.success(formatCompletionMessage(evt));
            }
            break;
          case "scan.failed":
            qc.invalidateQueries({
              queryKey: queryKeys.scanRunsAll(evt.library_id),
            });
            if (toastErrors && rememberScanToast(evt.scan_id)) {
              toast.error(`Scan failed: ${evt.error}`);
            }
            break;
          case "scan.health_issue":
            qc.invalidateQueries({
              queryKey: queryKeys.health(evt.library_id),
            });
            if (toastErrors && evt.severity === "error") {
              toast.error(
                `Health issue: ${evt.kind}${evt.path ? ` — ${evt.path}` : ""}`,
              );
            }
            break;
          case "scan.started":
            qc.invalidateQueries({
              queryKey: queryKeys.scanRunsAll(evt.library_id),
            });
            break;
          case "thumbs.started":
          case "thumbs.completed":
          case "thumbs.failed":
            // Re-poll thumbnail status and queue depth as soon as worker
            // activity starts, then again as each job finishes.
            qc.invalidateQueries({
              queryKey: queryKeys.thumbnailsStatus(evt.library_id),
            });
            qc.invalidateQueries({ queryKey: queryKeys.queueDepth });
            if (toastErrors && evt.type === "thumbs.failed") {
              toast.error(`Thumbnail job failed: ${evt.error}`);
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
  if (parts.length === 0) parts.push("no changes");
  return `Scan complete · ${parts.join(", ")}`;
}
