"use client";

import { usePathname, useSearchParams } from "next/navigation";
import type * as React from "react";

/**
 * Picks the `(library)` group fallback by the URL a `loading.tsx` can't
 * see: bare `/` is the Home rails page, while `/?library=…` (and any
 * `(library)` route without its own `loading.tsx`) is a cover grid.
 * `useSearchParams` is safe in this fallback because the caller wraps it
 * in `<Suspense fallback={grid}>`: a static prerender of the loading
 * shell bails out to the grid instead of failing the build, and on client
 * navigation the params are available synchronously so the right shape
 * renders straight away.
 */
export function LibraryQuerySwitch({
  grid,
  home,
}: {
  grid: React.ReactNode;
  home: React.ReactNode;
}) {
  const pathname = usePathname();
  const params = useSearchParams();
  return pathname === "/" && !params.has("library") ? home : grid;
}
