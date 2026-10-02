"use client";

import * as React from "react";

/** Same-page broadcast for card-size changes (WP-7.7). Every
 *  `useCardSize` instance sharing a storage key listens, so the series
 *  page's Related tab covers follow the Issues panel's slider live. Other
 *  browser tabs hear about it through the native `storage` event. */
export const CARD_SIZE_EVENT = "folio:card-size";

export type CardSizeEventDetail = { key: string; value: number };

function clamp(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n));
}

/** Parse a stored value; `null` when absent or garbage. */
export function parseStoredCardSize(
  raw: string | null,
  min: number,
  max: number,
): number | null {
  if (raw == null || raw === "") return null;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed)) return null;
  return clamp(parsed, min, max);
}

function readStored(key: string, min: number, max: number): number | null {
  try {
    return parseStoredCardSize(window.localStorage.getItem(key), min, max);
  } catch {
    // Private mode / blocked storage: fall back to the default.
    return null;
  }
}

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
 *
 *  WP-7.7: instances sharing `storageKey` stay in sync — the setter
 *  broadcasts a `CARD_SIZE_EVENT` on `window` (same page) and the
 *  `storage` event carries the change to other browser tabs.
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
    const stored = readStored(storageKey, min, max);
    // eslint-disable-next-line react-hooks/set-state-in-effect
    if (stored != null) setCardSize(stored);

    const onLocal = (e: Event) => {
      const detail = (e as CustomEvent<CardSizeEventDetail>).detail;
      if (!detail || detail.key !== storageKey) return;
      if (Number.isFinite(detail.value)) {
        setCardSize(clamp(detail.value, min, max));
      }
    };
    const onStorage = (e: StorageEvent) => {
      // `key === null` is a `localStorage.clear()` in another tab.
      if (e.key !== null && e.key !== storageKey) return;
      setCardSize(parseStoredCardSize(e.newValue, min, max) ?? defaultSize);
    };
    window.addEventListener(CARD_SIZE_EVENT, onLocal);
    window.addEventListener("storage", onStorage);
    return () => {
      window.removeEventListener(CARD_SIZE_EVENT, onLocal);
      window.removeEventListener("storage", onStorage);
    };
    // Only re-subscribe on key change (rare). Bounds are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey]);

  const persistAndSet = React.useCallback(
    (next: number) => {
      setCardSize(next);
      if (typeof window === "undefined") return;
      try {
        window.localStorage.setItem(storageKey, String(next));
      } catch {
        // Storage unavailable: the in-page broadcast still syncs.
      }
      window.dispatchEvent(
        new CustomEvent<CardSizeEventDetail>(CARD_SIZE_EVENT, {
          detail: { key: storageKey, value: next },
        }),
      );
    },
    [storageKey],
  );

  return [cardSize, persistAndSet] as const;
}
