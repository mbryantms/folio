"use client";
import { connectionFetch, reportConnectivity } from "@/lib/pwa/connectivity";

/**
 * Client-side `/api/*` fetch wrapper with implicit access-token renewal.
 *
 * Auth model recap (§17.2): access cookie is short (1h by default),
 * refresh cookie is 30d. Signed-in pages renew the access token before it
 * lapses (`session-refresh.ts`). When a request still meets an expired
 * access cookie (a sleeping laptop, a throttled background tab) it returns
 * 401; this helper then triggers a single `POST /api/auth/refresh` and
 * retries the original request once.
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

/** The readable access-expiry cookie, `<exp>.<lifetime>`: the current
 *  access token's `exp` (epoch seconds, server clock) and its lifetime in
 *  seconds. Set and cleared by the server with the session cookie. An older
 *  server sent `<exp>` alone. */
export const ACCESS_EXP_COOKIE = "__Host-comic_access_exp";

export type AccessExpiry = {
  /** `exp`, epoch seconds on the server's clock. */
  exp: number;
  /** The token's lifetime in seconds, `null` from the old one-number form. */
  lifetime: number | null;
};

/** Parse the access-expiry cookie, or `null` when it is absent (signed out,
 *  or the access token already lapsed: its Max-Age is the access lifetime)
 *  or unreadable. */
export function readAccessExpiry(): AccessExpiry | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(
    /(?:^|;\s*)__Host-comic_access_exp=([0-9]+)(?:\.([0-9]+))?(?:;|$)/,
  );
  if (!m) return null;
  const exp = Number(m[1]);
  if (!Number.isFinite(exp) || exp <= 0) return null;
  const lifetime = m[2] === undefined ? null : Number(m[2]);
  return {
    exp,
    lifetime:
      lifetime !== null && Number.isFinite(lifetime) && lifetime > 0
        ? lifetime
        : null,
  };
}

/** The access token's expiry in epoch seconds (server clock), or `null`. */
export function readAccessExp(): number | null {
  return readAccessExpiry()?.exp ?? null;
}

/** Where the last measured offset is kept for this tab, so a page load
 *  starts from it instead of from zero (a client clock far ahead would
 *  otherwise cost one early refresh on every load). */
const CLOCK_OFFSET_KEY = "folio:server-clock-offset";

function storedClockOffset(): number {
  try {
    const raw = globalThis.sessionStorage?.getItem(CLOCK_OFFSET_KEY);
    const n = raw === null || raw === undefined ? NaN : Number(raw);
    return Number.isFinite(n) ? n : 0;
  } catch {
    return 0;
  }
}

/** Estimated `server time - client time` in ms, from the newest `Date`
 *  response header seen: this tab's last measurement, or zero until a
 *  response carried one. */
let clockOffsetMs = storedClockOffset();

/** Learn the server's clock from a response's `Date` header (ignored when
 *  missing or unparsable). `Date` has one-second resolution and is
 *  truncated, so the estimate takes the middle of that second. */
export function noteServerDate(res: Response, receivedAt = Date.now()): void {
  const raw = res.headers?.get?.("date");
  if (!raw) return;
  const serverMs = Date.parse(raw);
  if (!Number.isFinite(serverMs)) return;
  const next = serverMs + 500 - receivedAt;
  // `Date` has one-second resolution: only a real change is worth a write.
  const moved = Math.abs(next - clockOffsetMs) >= 1000;
  clockOffsetMs = next;
  if (!moved) return;
  try {
    globalThis.sessionStorage?.setItem(CLOCK_OFFSET_KEY, String(next));
  } catch {
    /* storage unavailable: the offset is relearned on the next load */
  }
}

/** "Now" on the server's clock, in ms: every comparison against the expiry
 *  cookie (which the server wrote) uses this, never the bare client clock. */
export function serverNow(): number {
  return Date.now() + clockOffsetMs;
}

/** Test hook: forget the measured offset. */
export function resetClockOffsetForTests(): void {
  clockOffsetMs = 0;
  try {
    globalThis.sessionStorage?.removeItem(CLOCK_OFFSET_KEY);
  } catch {
    /* nothing stored */
  }
}

/**
 * What one refresh attempt came to.
 * - `renewed`: this tab has working cookies (it refreshed, or another tab did).
 * - `signed-out`: the server refused the session (or no CSRF cookie).
 * - `answered`: the server answered with something else (403, 5xx); nothing
 *   is known about the session.
 * - `no-answer`: no HTTP answer at all (timeout or network error). The
 *   server may still have rotated the refresh token; it accepts the
 *   previous token once for a minute after a rotation, so the caller should
 *   retry soon (`session-refresh.ts` does).
 */
export type RefreshOutcome =
  "renewed" | "signed-out" | "answered" | "no-answer";

let refreshInFlight: Promise<RefreshOutcome> | null = null;

/** Web Locks name serializing `/auth/refresh` across this origin's tabs. */
const REFRESH_LOCK = "folio:auth-refresh";
/** How long a tab that lost a refresh race waits for the winner's
 *  Set-Cookie to land before concluding it is signed out. */
const LOST_RACE_SETTLE_MS = 300;
/** Upper bound on one `/auth/refresh` round trip. A hung request is "no
 *  answer" (`null`), never a sign-out. */
export const REFRESH_TIMEOUT_MS = 15_000;

type RenewedListener = () => void;
const renewedListeners = new Set<RenewedListener>();

/** Subscribe to "this tab's cookies were renewed" (by this tab's refresh, or
 *  another tab's that this tab noticed). Returns the unsubscribe. */
export function onSessionRenewed(listener: RenewedListener): () => void {
  renewedListeners.add(listener);
  return () => renewedListeners.delete(listener);
}

type SettledListener = (outcome: RefreshOutcome, sentAt: number | null) => void;
const settledListeners = new Set<SettledListener>();

/**
 * Subscribe to "a refresh attempt in this tab settled", whoever started it
 * (the scheduler, `ensureFreshSession`, the 401 retry in `apiFetch`).
 * Called after the attempt is no longer in flight, so a listener that
 * starts a new refresh gets a new request rather than the settled one.
 * `sentAt` is the client time (`Date.now()`) just before the request went
 * out, which is no later than any rotation the server may have made for
 * it; `null` when no request was sent (another tab had refreshed, or no
 * CSRF cookie).
 */
export function onRefreshSettled(listener: SettledListener): () => void {
  settledListeners.add(listener);
  return () => settledListeners.delete(listener);
}

function notifySettled(outcome: RefreshOutcome, sentAt: number | null) {
  for (const l of settledListeners) {
    try {
      l(outcome, sentAt);
    } catch {
      /* a listener's failure must not break the refresh */
    }
  }
}

function notifyRenewed() {
  for (const l of renewedListeners) {
    try {
      l();
    } catch {
      /* a listener's failure must not break the refresh */
    }
  }
}

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

/** Expire the script-readable auth cookies after the server definitively
 *  refused the session, so nothing (the sign-in page's resume, the refresh
 *  scheduler) keeps trying a dead session. The HttpOnly refresh cookie is
 *  out of reach; without the CSRF cookie it is never presented again. */
function expireReadableAuthCookies() {
  if (typeof document === "undefined") return;
  for (const name of ["__Host-comic_csrf", ACCESS_EXP_COOKIE]) {
    document.cookie = `${name}=; Max-Age=0; Path=/; Secure; SameSite=Lax`;
  }
}

/**
 * Renew the access token through `POST /api/auth/refresh`.
 *
 * Resolves to a `RefreshOutcome`. `renewed`: this tab now has working
 * cookies (it refreshed, or another tab did). `signed-out`: the session is
 * gone. `answered` (a 403 or 5xx) and `no-answer` (unreachable, or nothing
 * within `REFRESH_TIMEOUT_MS`): nothing is known and nothing is signed out;
 * every outcome reaches `onRefreshSettled` listeners once it has settled.
 *
 * Refresh-token rotation is single-use on the server, except that the
 * token a rotation replaced is accepted once more within a minute (a lost
 * response can be retried; that retry retires the token the lost response
 * carried). Of two tabs that present the same refresh cookie at once, the
 * second usually gets a pair too, and a third gets 401. Cookies are shared
 * by every tab, so a tab that gets 401 is not signed out: the winner's
 * Set-Cookie already gave it the new tokens. Whether
 * another tab refreshed is read from the access-expiry cookie: it counts
 * only when the cookie is present, later than the value this tab saw
 * before it tried, and still in the future. A missing cookie never counts
 * (sign-out clears it). Two layers:
 *   - With Web Locks, refreshes are serialized across tabs; a tab that
 *     acquires the lock after another tab refreshed sees the later expiry
 *     and skips its own refresh.
 *   - Without them (or if the race still happens), a 401 from the
 *     refresh is a lost race when the expiry moved, checked again once
 *     after a short wait for the winner's response.
 * Only a 401 is a refusal. A 403 is a CSRF failure (the cookie changed
 * under the request), which says nothing about the session: `answered`.
 * "Future" is judged on the server's clock (`serverNow`).
 */
function attemptRefresh(): Promise<RefreshOutcome> {
  if (refreshInFlight) return refreshInFlight;
  const before = readAccessExp();
  const someoneElseRefreshed = () => {
    const now = readAccessExp();
    if (now === null) return false;
    if (before !== null && now <= before) return false;
    return now * 1000 > serverNow();
  };
  let sentAt: number | null = null;
  const p = withRefreshLock(async (): Promise<RefreshOutcome> => {
    if (someoneElseRefreshed()) return "renewed";
    const csrf = getCsrfToken();
    // Without the CSRF cookie the refresh cannot pass the CSRF check: the
    // session is gone (sign-out cleared it).
    if (!csrf) return "signed-out";
    sentAt = Date.now();
    try {
      const res = await connectionFetch("/api/auth/refresh", {
        method: "POST",
        credentials: "include",
        headers: { "X-CSRF-Token": csrf },
        signal: AbortSignal.timeout(REFRESH_TIMEOUT_MS),
      });
      noteServerDate(res);
      if (res.ok) return "renewed";
      if (res.status !== 401) return "answered";
      if (someoneElseRefreshed()) return "renewed";
      await new Promise((resolve) => setTimeout(resolve, LOST_RACE_SETTLE_MS));
      if (someoneElseRefreshed()) return "renewed";
      // Refused, and the expiry did not move: the session is gone.
      expireReadableAuthCookies();
      return "signed-out";
    } catch {
      // Network failure or timeout: no answer.
      return "no-answer";
    }
  });
  refreshInFlight = p;
  // One reaction, registered before any caller's: the in-flight slot is
  // cleared first, then listeners run, then callers resume.
  void p.then(
    (outcome) => {
      if (refreshInFlight === p) refreshInFlight = null;
      if (outcome === "renewed") notifyRenewed();
      notifySettled(outcome, sentAt);
    },
    () => {
      if (refreshInFlight === p) refreshInFlight = null;
    },
  );
  return p;
}

/** Renew the session now through the cross-tab path (deduped with any
 *  refresh already in flight). Same result as `attemptRefresh`. */
export function refreshSession(): Promise<boolean | null> {
  return attemptRefresh().then(asTriState);
}

/** `refreshSession` with the outcome spelled out (see `RefreshOutcome`). */
export function refreshSessionOutcome(): Promise<RefreshOutcome> {
  return attemptRefresh();
}

function asTriState(outcome: RefreshOutcome): boolean | null {
  if (outcome === "renewed") return true;
  if (outcome === "signed-out") return false;
  return null;
}

/**
 * Resume a session whose access cookie has expired, for the sign-in page a
 * full page load was redirected to. Only tries when the CSRF cookie (which
 * lives as long as the refresh cookie, and is cleared by sign-out) shows a
 * session may exist, so a signed-out visitor sends nothing.
 */
export async function resumeSession(): Promise<boolean> {
  if (!getCsrfToken()) return false;
  return (await attemptRefresh()) === "renewed";
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
  noteServerDate(res);
  if (res.status !== 401) return res;
  // Don't try to refresh the refresh endpoint itself — that would loop.
  if (path === "/auth/refresh") return res;
  const refreshed = asTriState(await attemptRefresh());
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
