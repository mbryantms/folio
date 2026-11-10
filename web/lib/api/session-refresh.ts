"use client";
import {
  getCsrfToken,
  onSessionRenewed,
  readAccessExp,
  refreshSession,
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
 */

/** Never refresh later than this before the access token expires. */
export const MIN_LEAD_MS = 30_000;
/** Share of the remaining lifetime to wait before refreshing. */
const LEAD_FRACTION = 0.8;
/** After a refresh that got no answer, try again this much later. */
const RETRY_AFTER_NO_ANSWER_MS = 30_000;
/** `setTimeout` clamps larger delays to 0; cap and re-arm instead. */
const MAX_TIMER_MS = 2 ** 31 - 1;
/** Tabs announce a completed refresh here, so others re-arm at once. */
const RENEWED_CHANNEL = "folio:session-renewed";

/** Milliseconds until the scheduler should refresh, given the expiry. */
export function refreshDelayMs(expSeconds: number, now = Date.now()): number {
  const remaining = expSeconds * 1000 - now;
  const delay = Math.min(remaining * LEAD_FRACTION, remaining - MIN_LEAD_MS);
  return Math.max(0, delay);
}

/** True when the access token is missing or within `MIN_LEAD_MS` of
 *  expiring while a session may exist (a CSRF cookie is present). */
export function needsRefresh(now = Date.now()): boolean {
  if (!getCsrfToken()) return false;
  const exp = readAccessExp();
  return exp === null || exp * 1000 - now <= MIN_LEAD_MS;
}

function isOffline(): boolean {
  return typeof navigator !== "undefined" && navigator.onLine === false;
}

/**
 * Resolve once the access token is fresh enough to use: at once when it is
 * (or when no session exists), otherwise after a refresh. Resolves promptly
 * when the browser is offline or the refresh fails on the network; it never
 * rejects, so callers can simply `await` it before retrying a request.
 */
export async function ensureFreshSession(): Promise<void> {
  if (isOffline() || !needsRefresh()) return;
  try {
    await refreshSession();
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
    firing = true;
    let result: boolean | null = null;
    try {
      result = await refreshSession();
    } finally {
      firing = false;
    }
    if (stopped) return;
    if (result === true) arm();
    else if (result === null) schedule(RETRY_AFTER_NO_ANSWER_MS);
    // `false`: signed out; the readable cookies are gone, nothing to arm.
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
    if (document.visibilityState === "visible") arm();
  };
  document.addEventListener("visibilitychange", onVisible);
  window.addEventListener("online", arm);

  arm();

  return () => {
    stopped = true;
    clear();
    unsubscribe();
    channel?.close();
    document.removeEventListener("visibilitychange", onVisible);
    window.removeEventListener("online", arm);
  };
}
