"use client";

import { useSyncExternalStore } from "react";

/** Tailwind's `sm` breakpoint is 640px; anything narrower is "phone"
 *  for layout purposes (the reader chrome, toasts, the grid controls
 *  and page headers all branch on the same line). */
export const PHONE_QUERY = "(max-width: 639px)";

function subscribe(cb: () => void): () => void {
  const mq = window.matchMedia(PHONE_QUERY);
  mq.addEventListener?.("change", cb);
  return () => mq.removeEventListener?.("change", cb);
}

function getSnapshot(): boolean {
  return window.matchMedia(PHONE_QUERY).matches;
}

/** True below Tailwind's `sm` breakpoint. SSR (and the first client
 *  render) report `false` so server and client markup agree; the swap
 *  to the phone composition happens on hydration. Callers that can
 *  express the difference in CSS alone (`sm:hidden`, `max-sm:`) should
 *  keep doing so — use this only where the *component tree* differs. */
export function useIsPhone(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, () => false);
}
