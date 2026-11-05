"use client";

import { useCallback, useState } from "react";

function sameOrder(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((id, i) => id === b[i]);
}

/**
 * The order a sortable list renders after a drop, until the server agrees.
 *
 * `hold(next)` pins the dropped order. It is released automatically on the
 * first render where the server's order matches, not when the save
 * succeeds: clearing on success let the stale cached order paint for a
 * frame or two before the refetch arrived, so a dropped item flashed back
 * to its old slot. `release()` drops it at once (save failed);
 * `releaseLater()` lets the server win after a moment if it settled on a
 * different order (a concurrent edit).
 */
export function useHeldOrder(serverIds: string[]) {
  const [held, setHeld] = useState<string[] | null>(null);
  if (held && sameOrder(held, serverIds)) setHeld(null);
  const hold = useCallback((next: string[]) => setHeld(next), []);
  const release = useCallback(() => setHeld(null), []);
  const releaseLater = useCallback((next: string[], ms = 4000) => {
    setTimeout(() => setHeld((cur) => (cur === next ? null : cur)), ms);
  }, []);
  return { order: held ?? serverIds, held, hold, release, releaseLater };
}
