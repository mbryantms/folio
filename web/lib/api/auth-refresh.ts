"use client";
import { connectionFetch, reportConnectivity } from "@/lib/pwa/connectivity";

/**
 * Client-side `/api/*` fetch wrapper with implicit access-token renewal.
 *
 * Auth model recap (§17.2): access cookie is short (1h by default),
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

let refreshInFlight: Promise<boolean | null> | null = null;

/** Web Locks name serializing `/auth/refresh` across this origin's tabs. */
const REFRESH_LOCK = "folio:auth-refresh";
/** How long a tab that lost a refresh race waits for the winner's
 *  Set-Cookie to land before concluding it is signed out. */
const LOST_RACE_SETTLE_MS = 300;

/** Run `fn` holding the cross-tab refresh lock where Web Locks exist;
 *  otherwise (old browsers, tests) just run it. */
function withRefreshLock<T>(fn: () => Promise<T>): Promise<T> {
  const locks =
    typeof navigator !== "undefined"
      ? (navigator as Navigator & { locks?: LockManager }).locks
      : undefined;
  if (!locks?.request) return fn();
  return locks.request(REFRESH_LOCK, fn).catch(() => fn());
}

/**
 * Renew the access token through `POST /api/auth/refresh`.
 *
 * `true`: this tab now has working cookies (it refreshed, or another tab
 * did). `false`: the session is gone. `null`: the server could not be
 * reached or failed; nothing is known.
 *
 * Refresh-token rotation is single-use on the server: of two tabs that
 * present the same refresh cookie at once, exactly one wins and the other
 * gets 401. Cookies are shared by every tab, so the loser is not signed
 * out: the winner's Set-Cookie already gave it the new tokens. The CSRF
 * cookie (readable here, unlike the HttpOnly session cookies) is rotated
 * by every successful refresh, so a CSRF value that differs from the one
 * seen before refreshing means another tab refreshed. Two layers:
 *   - With Web Locks, refreshes are serialized across tabs; a tab that
 *     acquires the lock after another tab refreshed sees the new CSRF
 *     value and skips its own refresh.
 *   - Without them (or if the race still happens), a 401/403 from the
 *     refresh is treated as a lost race when the CSRF value changed,
 *     checked again once after a short wait for the winner's response.
 */
async function attemptRefresh(): Promise<boolean | null> {
  if (refreshInFlight) return refreshInFlight;
  const before = getCsrfToken();
  const someoneElseRefreshed = () => getCsrfToken() !== before;
  const p = withRefreshLock(async () => {
    if (someoneElseRefreshed()) return true;
    try {
      const csrf = getCsrfToken();
      const res = await connectionFetch("/api/auth/refresh", {
        method: "POST",
        credentials: "include",
        headers: csrf ? { "X-CSRF-Token": csrf } : undefined,
      });
      if (res.ok) return true;
      if (res.status !== 401 && res.status !== 403) return null;
      if (someoneElseRefreshed()) return true;
      await new Promise((resolve) => setTimeout(resolve, LOST_RACE_SETTLE_MS));
      return someoneElseRefreshed();
    } catch {
      return null;
    }
  });
  refreshInFlight = p;
  void p.finally(() => {
    if (refreshInFlight === p) refreshInFlight = null;
  });
  return p;
}

/**
 * Resume a session whose access cookie has expired, for the sign-in page a
 * full page load was redirected to. Only tries when the CSRF cookie (which
 * lives as long as the refresh cookie, and is cleared by sign-out) shows a
 * session may exist, so a signed-out visitor sends nothing.
 */
export async function resumeSession(): Promise<boolean> {
  if (!getCsrfToken()) return false;
  return (await attemptRefresh()) === true;
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
  const res = await connectionFetch(url, opts);
  if (res.status !== 401) return res;
  // Don't try to refresh the refresh endpoint itself — that would loop.
  if (path === "/auth/refresh") return res;
  const refreshed = await attemptRefresh();
  if (!refreshed) {
    if (refreshed === false) reportConnectivity("authentication");
    return res;
  }
  // Re-read the CSRF cookie before the retry. `/api/auth/refresh` and
  // any token_version-bumping mutation (password change, sign-out-
  // everywhere) rotate `__Host-comic_csrf` in its Set-Cookie, so the
  // original `init.headers['X-CSRF-Token']` is now stale. Retrying
  // with the stale header sends the new cookie + old header → 403.
  // Surfaced as the "CSRF token missing or mismatched" loop seen
  // after a successful password change in the same session.
  const retryOpts: RequestInit = { ...opts };
  const csrf = getCsrfToken();
  if (csrf) {
    const headers = new Headers(opts.headers ?? {});
    headers.set("X-CSRF-Token", csrf);
    retryOpts.headers = headers;
  }
  return connectionFetch(url, retryOpts);
}
