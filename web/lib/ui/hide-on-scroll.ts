/**
 * Bottom-tab-bar hide-on-scroll decision (folio-flair, standard).
 *
 * Pure so it can be unit-tested: the hook feeds it each scroll position
 * and gets back whether the bar should be hidden. Direction is decided on
 * *accumulated* travel in one direction, not per event, so a jittery
 * finger or momentum wobble doesn't flicker the bar: it hides after
 * `threshold` px of continuous downward travel and returns after the same
 * upward. It is always shown near the top (nothing to make room for), at
 * the page end (the content's bottom padding was sized for the bar, so the
 * last row sits right above it), and whenever the surface is `blocked`
 * (a sheet / menu / dialog is open).
 */
export interface HideOnScrollState {
  hidden: boolean;
  /** Signed px travelled in the current direction. */
  acc: number;
  lastY: number;
}

export interface HideOnScrollInput {
  /** Current scroll offset. May be negative during iOS rubber-banding. */
  y: number;
  /** Largest reachable offset (`scrollHeight - innerHeight`). */
  maxY: number;
  /** An overlay is open: never hide, and reset the accumulator. */
  blocked: boolean;
}

export const HIDE_ON_SCROLL_DEFAULTS = {
  /** Px of one-way travel that flips the bar. */
  threshold: 24,
  /** Within this many px of the top the bar always shows. */
  topZone: 40,
  /** Within this many px of the end the bar always shows. */
  bottomZone: 24,
} as const;

export function initialHideOnScrollState(y = 0): HideOnScrollState {
  return { hidden: false, acc: 0, lastY: y };
}

export function nextHideOnScroll(
  prev: HideOnScrollState,
  { y, maxY, blocked }: HideOnScrollInput,
  opts: Partial<typeof HIDE_ON_SCROLL_DEFAULTS> = {},
): HideOnScrollState {
  const { threshold, topZone, bottomZone } = {
    ...HIDE_ON_SCROLL_DEFAULTS,
    ...opts,
  };
  if (blocked) return { hidden: false, acc: 0, lastY: y };
  const dy = y - prev.lastY;
  // Same direction keeps accumulating; a reversal starts over from dy.
  const acc =
    dy === 0
      ? prev.acc
      : Math.sign(dy) === Math.sign(prev.acc)
        ? prev.acc + dy
        : dy;
  if (y <= topZone || (maxY > topZone && y >= maxY - bottomZone)) {
    return { hidden: false, acc, lastY: y };
  }
  let hidden = prev.hidden;
  if (acc >= threshold) hidden = true;
  else if (acc <= -threshold) hidden = false;
  return { hidden, acc, lastY: y };
}
