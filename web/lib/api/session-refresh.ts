"use client";
import {
  getCsrfToken,
  onRefreshSettled,
  onSessionRenewed,
  readAccessExp,
  readAccessExpiry,
  refreshSessionOutcome,
  serverNow,
  type RefreshOutcome,
} from "./auth-refresh";

/**
 * Proactive access-token renewal for signed-in pages.
 *
 * The server sets `__Host-comic_access_exp` (the access token's expiry,
 * epoch seconds, readable by scripts) next to the HttpOnly session cookie.
 * The scheduler reads it and renews the session through the same
 * cross-tab path a 401 uses (`refreshSessionOutcome`) before the token lapses, so
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
 *
 * No answer: a refresh that timed out or failed on the network may still
 * have rotated the refresh token on the server, which then accepts the
 * previous token once for 60 s after the rotation (`REFRESH_GRACE` in
 * `auth/local.rs`). Such an attempt does not count against the floor, and
 * the scheduler retries it at once and once more `FAST_RETRY_GAP_MS` later,
 * each only while less than `FAST_RETRY_HORIZON_MS` has passed since the
 * unanswered request was sent; then the ordinary backoff. With the 15 s
 * request timeout: sent at 0, timeout at 15 s, retry at 15 s, its timeout
 * at 30 s, retry at 35 s, its timeout at 50 s, then 30 s, 60 s, ... later.
 * While the last attempt is unanswered, coming back online or becoming
 * visible retries at once if a fast retry is left in the budget and the
 * horizon, spending it; otherwise it leaves the backoff alone. A refresh
 * from any path (the 401 retry, `ensureFreshSession`) counts: no answer
 * starts the fast retries once that attempt has settled, and any answer
 * ends them. The floor still holds after any HTTP answer.
 */

/** Never refresh later than this before the access token expires. */
export const MIN_LEAD_MS = 30_000;
/** Share of the remaining lifetime to wait before refreshing. */
const LEAD_FRACTION = 0.8;
/** After a refresh that got no answer, try again this much later, doubling
 *  up to `MAX_NO_ANSWER_BACKOFF_MS` while the server stays silent. */
const RETRY_AFTER_NO_ANSWER_MS = 30_000;
const MAX_NO_ANSWER_BACKOFF_MS = 10 * 60_000;
/** Fast retries after a refresh with no answer (see the module comment):
 *  at most this many, the first at once, the next this much later, and none
 *  sent later than the horizon after the unanswered request was sent. The
 *  horizon leaves 20 s of the server's 60 s grace for the request to arrive. */
export const MAX_FAST_RETRIES = 2;
export const FAST_RETRY_GAP_MS = 5_000;
export const FAST_RETRY_HORIZON_MS = 40_000;
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

/** Start a proactive refresh, recording it for the floor. An attempt that
 *  got no answer is taken off the floor again: it may never have reached the
 *  server, and its retry must not wait out the floor. */
async function proactiveRefresh(): Promise<RefreshOutcome> {
  const previous = lastProactiveAt;
  const startedAt = Date.now();
  lastProactiveAt = startedAt;
  const outcome = await refreshSessionOutcome();
  if (outcome === "no-answer" && lastProactiveAt === startedAt) {
    lastProactiveAt = previous;
  }
  return outcome;
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
  /** Client time the first still-unanswered refresh was sent, or null. */
  let unansweredSince: number | null = null;
  /** Fast retries spent on the current unanswered refresh. */
  let fastRetriesUsed = 0;
  let firing = false;

  const clear = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };

  const schedule = (delay: number) => {
    clear();
    if (stopped) return;
    timer = setTimeout(
      () => {
        timer = null;
        void fire();
      },
      Math.min(delay, MAX_TIMER_MS),
    );
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

  const backoff = () => {
    schedule(noAnswerDelay);
    noAnswerDelay = Math.min(noAnswerDelay * 2, MAX_NO_ANSWER_BACKOFF_MS);
  };

  /** True while a fast retry `delay` ms from now is still in the budget:
   *  retries left, and sent within the horizon of the unanswered request. */
  const fastRetryAllowed = (delay: number) =>
    unansweredSince !== null &&
    fastRetriesUsed < MAX_FAST_RETRIES &&
    Date.now() + delay - unansweredSince <= FAST_RETRY_HORIZON_MS;

  /** A refresh got no answer: retry fast while that can still land inside
   *  the server's grace window, then back off. */
  const afterNoAnswer = () => {
    const delay = fastRetriesUsed === 0 ? 0 : FAST_RETRY_GAP_MS;
    if (fastRetryAllowed(delay)) {
      fastRetriesUsed += 1;
      schedule(delay);
      return;
    }
    backoff();
  };

  /** Some HTTP answer arrived (or another tab renewed): not unanswered. */
  const answered = () => {
    unansweredSince = null;
    fastRetriesUsed = 0;
  };

  const fire = async () => {
    if (stopped || firing) return;
    if (!getCsrfToken()) return;
    // Another tab may have refreshed since this timer was armed: the
    // expiry moved later, so arm for the new one instead.
    const exp = readAccessExp();
    if (exp !== null && armedFor !== null && exp > armedFor) {
      answered();
      arm();
      return;
    }
    if (isOffline()) return; // `online` retries
    // The floor holds whatever the clocks say, except for the bounded
    // retries of an attempt that got no answer.
    if (unansweredSince === null) {
      const wait = floorWaitMs();
      if (wait > 0) {
        schedule(wait);
        return;
      }
    }
    firing = true;
    let outcome: RefreshOutcome;
    try {
      outcome = await proactiveRefresh();
    } finally {
      firing = false;
    }
    if (stopped) return;
    if (outcome === "no-answer") {
      afterNoAnswer();
      return;
    }
    answered();
    if (outcome === "renewed") {
      noAnswerDelay = RETRY_AFTER_NO_ANSWER_MS;
      arm();
    } else if (outcome === "answered") {
      backoff();
    }
    // `signed-out`: the readable cookies are gone, nothing to arm.
  };

  /** Visible again or back online: the server may be back, so retry soon
   *  instead of waiting out the backoff. While the last refresh is
   *  unanswered, retry at once only within the fast-retry budget (it spends
   *  one); otherwise leave the backoff as it is, and only make sure a timer
   *  is pending (an offline `fire()` arms none). */
  const rearmFresh = () => {
    if (unansweredSince !== null) {
      if (firing) return;
      if (fastRetryAllowed(0)) {
        fastRetriesUsed += 1;
        clear();
        void fire();
      } else if (timer === null) {
        backoff();
      }
      return;
    }
    noAnswerDelay = RETRY_AFTER_NO_ANSWER_MS;
    arm();
  };

  // Every settled refresh in this tab, whoever started it (this
  // scheduler, the 401 retry, `ensureFreshSession`). Listeners run after
  // the attempt has left the in-flight slot, so a retry scheduled here is a
  // new request. An answer of any kind ends the unanswered state; no
  // answer starts or continues it. This scheduler's own attempt is handled
  // when it returns (`firing`).
  const unsubscribeSettled = onRefreshSettled((outcome, sentAt) => {
    if (outcome !== "no-answer") {
      answered();
      return;
    }
    if (unansweredSince === null && sentAt !== null) {
      unansweredSince = sentAt;
      fastRetriesUsed = 0;
    }
    if (!firing && !stopped) afterNoAnswer();
  });

  const channel =
    typeof BroadcastChannel !== "undefined"
      ? new BroadcastChannel(RENEWED_CHANNEL)
      : null;
  if (channel)
    channel.onmessage = () => {
      answered();
      arm();
    };

  const unsubscribe = onSessionRenewed(() => {
    channel?.postMessage("renewed");
    answered();
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
    unsubscribeSettled();
    channel?.close();
    document.removeEventListener("visibilitychange", onVisible);
    window.removeEventListener("online", rearmFresh);
  };
}
