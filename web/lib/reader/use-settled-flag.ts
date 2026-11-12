"use client";

import * as React from "react";

/**
 * `value`, but a `true` only counts once it has stayed `true` for
 * `delayMs`; `false` is immediate. With `delayMs <= 0` it is just `value`.
 *
 * The reader uses it so a page passed while scrolling a webtoon never
 * starts a text detection: only a page that stays the current page for a
 * moment does (`WEBTOON_DETECT_DELAY_MS`).
 */
export function useSettledFlag(value: boolean, delayMs: number): boolean {
  const [settled, setSettled] = React.useState(false);
  React.useEffect(() => {
    if (!value || delayMs <= 0) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setSettled(false);
      return;
    }
    const t = setTimeout(() => setSettled(true), delayMs);
    return () => clearTimeout(t);
  }, [value, delayMs]);
  return value && (delayMs <= 0 || settled);
}
