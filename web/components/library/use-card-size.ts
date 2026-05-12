"use client";

import * as React from "react";

/** Hook + storage key handling for the card-size sliders that live on
 *  every page with a cover grid (series issues, saved-view detail).
 *
 *  Returns `[cardSize, setCardSize]`. Initial render uses `defaultSize`
 *  so SSR markup stays stable; a mount-time effect rehydrates from
 *  `localStorage`. Persistence is wired into the setter rather than a
 *  separate effect — a mount-time persist effect would race the
 *  rehydrate effect and clobber the saved value with the default
 *  before the rehydrated state landed, so adjustments wouldn't survive
 *  a page reload (especially under StrictMode's effect re-fire).
 */
export function useCardSize(opts: {
  storageKey: string;
  min: number;
  max: number;
  defaultSize: number;
}): readonly [number, (next: number) => void] {
  const { storageKey, min, max, defaultSize } = opts;
  const [cardSize, setCardSize] = React.useState(defaultSize);

  React.useEffect(() => {
    if (typeof window === "undefined") return;
    const raw = window.localStorage.getItem(storageKey);
    if (!raw) return;
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setCardSize(Math.min(max, Math.max(min, parsed)));
    // Only re-hydrate on key change (rare). Bounds are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey]);

  const persistAndSet = React.useCallback(
    (next: number) => {
      setCardSize(next);
      if (typeof window !== "undefined") {
        window.localStorage.setItem(storageKey, String(next));
      }
    },
    [storageKey],
  );

  return [cardSize, persistAndSet] as const;
}
