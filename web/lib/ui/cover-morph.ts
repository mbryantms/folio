"use client";

import { useCallback, useId, useSyncExternalStore } from "react";

/**
 * Cover morph (folio-flair-1.0, standard): the cover you click on a card
 * flies into the cover slot of the page it opens.
 *
 * React pairs `<ViewTransition name=…>` elements across a navigation by
 * name. Two things shape this module:
 *
 * - A name that appears twice on a page cancels the whole transition, and
 *   the same series can sit in several rails at once. So a card carries
 *   the name only while it is the one just clicked: `onClick` marks this
 *   card instance (by `useId`), the card re-renders with the name before
 *   the navigation commits, and the mark clears shortly after.
 * - Detail pages are dynamic, so a navigation first commits the route's
 *   `loading.tsx` skeleton; a pair only forms if the skeleton holds the
 *   named element. The click therefore also records the cover URL, and
 *   `<MorphCoverSlot>` in the skeleton draws that cover under the same
 *   name. When the page streams in, the skeleton slot hands off to the
 *   real header cover (same name again).
 */
interface Active {
  token: string;
  name: string;
  src: string | null;
}
let active: Active | null = null;
let clearTimer: ReturnType<typeof setTimeout> | null = null;
const subs = new Set<() => void>();

function set(next: Active | null) {
  active = next;
  subs.forEach((f) => f());
}
function subscribe(cb: () => void) {
  subs.add(cb);
  return () => subs.delete(cb);
}
const getActive = () => active;
const getNull = () => null;

export { issueCoverName, seriesCoverName } from "./cover-morph-names";

/** For a card: the name to render (only while it was just clicked) and the
 *  click handler that marks it. */
export function useCoverMorph(name: string, src: string | null | undefined) {
  const token = useId();
  const isActive = useSyncExternalStore(
    subscribe,
    () => active?.token === token,
    () => false,
  );
  const onClick = useCallback(
    (e: React.MouseEvent) => {
      // New-tab / modified clicks don't navigate this page.
      if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0)
        return;
      if (clearTimer) clearTimeout(clearTimer);
      set({ token, name, src: src ?? null });
      clearTimer = setTimeout(() => set(null), 2500);
    },
    [token, name, src],
  );
  return { name: isActive ? name : undefined, onClick };
}

/** The cover currently in flight, for the loading skeleton. */
export function useActiveMorph(): Active | null {
  return useSyncExternalStore(subscribe, getActive, getNull);
}
