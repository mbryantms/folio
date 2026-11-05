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
 * - React tracks mounted names per `<ViewTransition>` fiber and only
 *   forgets one when that fiber unmounts. Flipping `name` on a mounted
 *   boundary (a card going name → undefined when the mark clears while it
 *   is still on screen, e.g. a navigation slower than the mark) leaves a
 *   stale entry, and the destination header then trips "two
 *   <ViewTransition name=…> mounted at the same time". So every
 *   carrier of a toggled name renders through `<CoverMorph>`, which keys
 *   the boundary by name: a name change is an unmount + mount.
 * - The reader has no header cover. Its loading fallback holds the cover
 *   (`ReaderCoverSlot`) and hands it to the reader through
 *   `holdReaderHandoff`, which keeps it over the first page until that
 *   page has decoded.
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

/** Mark a cover as in flight. `token` ties it to one card instance (its
 *  `useId`); surfaces that always carry the name (a detail page's header
 *  cover, under a Read button) pass none. */
export function markCoverMorph(
  name: string,
  src: string | null | undefined,
  token: string | null = null,
) {
  if (clearTimer) clearTimeout(clearTimer);
  set({ token: token ?? `mark-${name}`, name, src: src ?? null });
  clearTimer = setTimeout(() => set(null), 2500);
}
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
      markCoverMorph(name, src, token);
    },
    [token, name, src],
  );
  /** For taps that navigate programmatically (play overlays, sheets). */
  const mark = useCallback(
    () => markCoverMorph(name, src, token),
    [token, name, src],
  );
  return {
    name: isActive ? name : undefined,
    /** Always set: where the in-app Back flight lands (`CoverMorph`'s
     *  `landing`). */
    landing: name || undefined,
    onClick,
    mark,
  };
}

/** The cover currently in flight, for the loading skeleton. */
export function useActiveMorph(): Active | null {
  return useSyncExternalStore(subscribe, getActive, getNull);
}

/** Cover handed from the reader's loading fallback to the reader. Written
 *  by `ReaderCoverSlot` once it shows a cover, read by
 *  `ReaderCoverHandoff` when the reader mounts, cleared when it lets go. */
let readerHandoff: Active | null = null;
export function holdReaderHandoff(m: Active | null) {
  readerHandoff = m;
}
export function peekReaderHandoff(): Active | null {
  return readerHandoff;
}
export function releaseReaderHandoff(m: Active) {
  if (readerHandoff === m) readerHandoff = null;
}
export type { Active as CoverMorphFlight };
