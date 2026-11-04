"use client";

import { useSyncExternalStore } from "react";

const noop = () => () => {};

/**
 * False on the server and during hydration, true after. Gate anything
 * rendered from a query the admin shell (or a restored offline cache) may
 * already have warm, so the server HTML and the first client render agree.
 */
export function useHydrated(): boolean {
  return useSyncExternalStore(
    noop,
    () => true,
    () => false,
  );
}
