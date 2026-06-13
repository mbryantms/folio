/**
 * One-time reader first-run coachmark state (audit C5).
 *
 * The reader's whole interaction model — tap zones, swipe-to-turn,
 * double-tap zoom, the keyboard map — was previously undiscoverable: a
 * new user landed on a full-bleed page with no affordances. This tracks
 * whether they've dismissed the orientation overlay, *globally* (not
 * per-series): you only need to learn the gestures once.
 *
 * Versioned so a future overlay redesign can re-show without colliding
 * with the old flag — bump `v1` → `v2` when the content materially
 * changes. Storage access is wrapped because Safari private mode throws
 * on `setItem`; we fail toward "already seen" so a broken localStorage
 * never traps the user behind an overlay we can't remember dismissing.
 */
export const READER_FIRST_RUN_KEY = "reader:firstRunSeen:v1";

export function hasSeenReaderFirstRun(): boolean {
  if (typeof window === "undefined") return true;
  try {
    return window.localStorage.getItem(READER_FIRST_RUN_KEY) === "1";
  } catch {
    return true;
  }
}

export function markReaderFirstRunSeen(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(READER_FIRST_RUN_KEY, "1");
  } catch {
    // ignore — worst case the overlay reappears next session.
  }
}

/**
 * `useSyncExternalStore` plumbing so the overlay can read the flag
 * SSR-safely (no hydration flash, no `setState`-in-effect). The flag
 * only changes via our own dismissal — handled by local state in the
 * reader — so there is nothing external to subscribe to. The server
 * snapshot is "seen" so the overlay never renders during SSR.
 */
export function subscribeReaderFirstRun(): () => void {
  return () => undefined;
}

export function readerFirstRunServerSnapshot(): boolean {
  return true;
}
