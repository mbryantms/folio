"use client";
import {
  getCsrfToken,
  onSessionRenewed,
  readAccessExp,
  readAccessExpiry,
  refreshSession,
  serverNow,
} from "./auth-refresh";

/**
 * Proactive access-token renewal for signed-in pages.
 *
 * The server sets `__Host-comic_access_exp` (the access token's expiry,
 * epoch seconds, readable by scripts) next to the HttpOnly session cookie.
 * The scheduler reads it and renews the session through the same
 * cross-tab path a 401 uses (`refreshSession`) before the token lapses, so
 * requests the 401-retry in `apiFetch` cannot cover — page images, which
 * the browser loads itself, and offline-download asset fetches — never
 * meet an expired cookie in the normal case.
 *
 * When it fires: at about 80% of the remaining lifetime, and never later
 * than `MIN_LEAD_MS` before expiry. It re-arms after every refresh (this
 * tab's, or another tab's it hears about), when the page becomes visible
 * again (timers in background tabs are throttled, and a sleeping machine
 * misses them entirely) and when the browser comes back online. Without
 * the CSRF cookie (signed out) it does nothing and arms no timer.
 *
 * Clocks: the expiry is on the server's clock, so every comparison uses
 * `serverNow()` (the client clock plus the offset learned from response
 * `Date` headers). Whatever the clocks say, a proactive refresh (the
 * scheduler's or `ensureFreshSession`'s) never starts within
 * `minRefreshIntervalMs` of the previous one in this tab, so a wrong
 * offset costs at most one early refresh per interval.
 */

/** Never refresh later than this before the access token expires. */
export const MIN_LEAD_MS = 30_000;
/** Share of the remaining lifetime to wait before refreshing. */
const LEAD_FRACTION = 0.8;
/** After a refresh that got no answer, try again this much later, doubling
 *  up to `MAX_NO_ANSWER_BACKOFF_MS` while the server stays silent. */
const RETRY_AFTER_NO_ANSWER_MS = 30_000;
const MAX_NO_ANSWER_BACKOFF_MS = 10 * 60_000;
/** Floor between two proactive refreshes: at least this, and at least
 *  `MIN_INTERVAL_FRACTION` of the token's lifetime. */
const MIN_REFRESH_INTERVAL_MS = 60_000;
const MIN_INTERVAL_FRACTION = 0.1;
/** `setTimeout` clamps larger delays to 0; cap and re-arm instead. */
const MAX_TIMER_MS = 2 ** 31 - 1;
/** Tabs announce a completed refresh here, so others re-arm at once. */
const RENEWED_CHANNEL = "folio:session-renewed";

/** Milliseconds until the scheduler should refresh, given the expiry. */
export function refreshDelayMs(expSeconds: number, now = serverNow()): number {
  const remaining = expSeconds * 1000 - now;
  const delay = Math.min(remaining * LEAD_FRACTION, remaining - MIN_LEAD_MS);
  return Math.max(0, delay);
}

/** True when the access token is missing or within `MIN_LEAD_MS` of
 *  expiring while a session may exist (a CSRF cookie is present). */
export function needsRefresh(now = serverNow()): boolean {
  if (!getCsrfToken()) return false;
  const exp = readAccessExp();
  return exp === null || exp * 1000 - now <= MIN_LEAD_MS;
}

/** The floor between two proactive refreshes: max(60 s, 10% of the token's
 *  lifetime). The lifetime comes from the cookie, never from the client
 *  clock; the old one-number cookie form gets the 60 s floor. */
export function minRefreshIntervalMs(): number {
  const lifetime = readAccessExpiry()?.lifetime ?? null;
  return Math.max(
    MIN_REFRESH_INTERVAL_MS,
    lifetime === null ? 0 : lifetime * 1000 * MIN_INTERVAL_FRACTION,
  );
}

/** Client time (elapsed-time only) of this tab's last proactive refresh. */
let lastProactiveAt: number | null = null;

/** Milliseconds until a proactive refresh may start (0 = now). */
function floorWaitMs(): number {
  if (lastProactiveAt === null) return 0;
  const elapsed = Date.now() - lastProactiveAt;
  // A clock set backwards makes `elapsed` negative: don't wait on it.
  if (elapsed < 0) return 0;
  return Math.max(0, minRefreshIntervalMs() - elapsed);
}

/** Start a proactive refresh, recording it for the floor. */
function proactiveRefresh(): Promise<boolean | null> {
  lastProactiveAt = Date.now();
  return refreshSession();
}

/** Test hook: forget the last proactive refresh. */
export function resetRefreshFloorForTests(): void {
  lastProactiveAt = null;
}

function isOffline(): boolean {
  return typeof navigator !== "undefined" && navigator.onLine === false;
}

/**
 * Resolve once the access token is fresh enough to use: at once when it is
 * (or when no session exists), otherwise after a refresh. Resolves promptly
 * when the browser is offline or the refresh fails on the network; it never
 * rejects, so callers can simply `await` it before retrying a request.
 * Inside the refresh floor it resolves at once and lets the request go: a
 * token that really expired is then handled by the 401 path.
 */
export async function ensureFreshSession(): Promise<void> {
  if (isOffline() || !needsRefresh() || floorWaitMs() > 0) return;
  try {
    await proactiveRefresh();
  } catch {
    /* no answer: the caller retries with what it has */
  }
}

/**
 * Start the scheduler for this page. Returns the stop function. Mounted
 * once by `SessionRefresher` in the signed-in shell.
 */
export function startSessionRefresh(): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;
  /** The expiry the pending timer was armed for. */
  let armedFor: number | null = null;
  /** Current wait after a refresh with no answer (exponential backoff). */
  let noAnswerDelay = RETRY_AFTER_NO_ANSWER_MS;

  const clear = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };

  const schedule = (delay: number) => {
    clear();
    if (stopped) return;
    timer = setTimeout(() => void fire(), Math.min(delay, MAX_TIMER_MS));
  };

  /** Arm from the cookie; refresh at once when already due. */
  const arm = () => {
    clear();
    if (stopped || !getCsrfToken()) return;
    const exp = readAccessExp();
    if (exp === null) {
      // No expiry cookie while a session may exist: the token lapsed (or
      // predates the cookie). Refresh now rather than wait for a 401.
      void fire();
      return;
    }
    armedFor = exp;
    const delay = refreshDelayMs(exp);
    if (delay <= 0) void fire();
    else schedule(delay);
  };

  let firing = false;
  const fire = async () => {
    if (stopped || firing) return;
    if (!getCsrfToken()) return;
    // Another tab may have refreshed since this timer was armed: the
    // expiry moved later, so arm for the new one instead.
    const exp = readAccessExp();
    if (exp !== null && armedFor !== null && exp > armedFor) {
      arm();
      return;
    }
    if (isOffline()) return; // `online` re-arms
    // The floor holds whatever the clocks say.
    const wait = floorWaitMs();
    if (wait > 0) {
      schedule(wait);
      return;
    }
    firing = true;
    let result: boolean | null = null;
    try {
      result = await proactiveRefresh();
    } finally {
      firing = false;
    }
    if (stopped) return;
    if (result === true) {
      noAnswerDelay = RETRY_AFTER_NO_ANSWER_MS;
      arm();
    } else if (result === null) {
      schedule(noAnswerDelay);
      noAnswerDelay = Math.min(noAnswerDelay * 2, MAX_NO_ANSWER_BACKOFF_MS);
    }
    // `false`: signed out; the readable cookies are gone, nothing to arm.
  };

  /** Visible again or back online: the server may be back, so retry soon
   *  instead of waiting out the backoff. */
  const rearmFresh = () => {
    noAnswerDelay = RETRY_AFTER_NO_ANSWER_MS;
    arm();
  };

  const channel =
    typeof BroadcastChannel !== "undefined"
      ? new BroadcastChannel(RENEWED_CHANNEL)
      : null;
  if (channel) channel.onmessage = () => arm();

  const unsubscribe = onSessionRenewed(() => {
    channel?.postMessage("renewed");
    arm();
  });
  const onVisible = () => {
    if (document.visibilityState === "visible") rearmFresh();
  };
  document.addEventListener("visibilitychange", onVisible);
  window.addEventListener("online", rearmFresh);

  arm();

  return () => {
    stopped = true;
    clear();
    unsubscribe();
    channel?.close();
    document.removeEventListener("visibilitychange", onVisible);
    window.removeEventListener("online", rearmFresh);
  };
}
