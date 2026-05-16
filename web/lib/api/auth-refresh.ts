"use client";

/**
 * Client-side `/api/*` fetch wrapper with implicit access-token renewal.
 *
 * Auth model recap (§17.2): access cookie is short-ish (24h by default),
 * refresh cookie is 30d. When the access cookie expires, any cookie-authed
 * request returns 401. Without intervention the user gets bounced to
 * sign-in; with this helper, the next 401 transparently triggers a single
 * `POST /api/auth/refresh`, then retries the original request.
 *
 * Concurrency: many components fire requests in parallel (TanStack Query
 * does its own coalescing per query key but not across keys, so a fresh
 * page load can fan out 5–10 requests simultaneously). A module-level
 * promise dedupes those into a single refresh round-trip.
 *
 * Path convention: as of v0.2.1 the `/api/` prefix is back — it
 * namespaces the JSON backend away from the HTML route space. Many
 * Rust API routes share path shapes with Next pages (`/admin/users`,
 * `/series/{id}`, `/audit`, etc.); without the prefix they collide.
 * Caller passes the bare backend path (e.g., `/series/abc-123`); this
 * helper prepends `/api`. Server-side, `app::router()` mounts the
 * conflict-prone routes under `Router::nest("/api", ...)`.
 */

export function getCsrfToken(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)__Host-comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}

let refreshInFlight: Promise<boolean> | null = null;

async function attemptRefresh(): Promise<boolean> {
  if (refreshInFlight) return refreshInFlight;
  const p = (async () => {
    try {
      const csrf = getCsrfToken();
      const res = await fetch("/api/auth/refresh", {
        method: "POST",
        credentials: "include",
        headers: csrf ? { "X-CSRF-Token": csrf } : undefined,
      });
      return res.ok;
    } catch {
      return false;
    }
  })();
  refreshInFlight = p;
  void p.finally(() => {
    if (refreshInFlight === p) refreshInFlight = null;
  });
  return p;
}

/**
 * Fetch a path under the `/api` prefix with cookie credentials and
 * implicit 401-driven refresh. Mirrors `fetch`'s API but takes the
 * path *without* the `/api` prefix (it's added here) and forces
 * `credentials: "include"`.
 */
export async function apiFetch(
  path: string,
  init?: RequestInit,
): Promise<Response> {
  const url = `/api${path}`;
  const opts: RequestInit = { credentials: "include", ...init };
  const res = await fetch(url, opts);
  if (res.status !== 401) return res;
  // Don't try to refresh the refresh endpoint itself — that would loop.
  if (path === "/auth/refresh") return res;
  const refreshed = await attemptRefresh();
  if (!refreshed) return res;
  return fetch(url, opts);
}
