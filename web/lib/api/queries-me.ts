/**
 * `useMe`, the current-user query every client tree reads.
 *
 * A shard of `queries.ts` (re-exported from there): the root-layout
 * listeners and `lib/visual-effects.ts` sit in every page's first load,
 * reader included, and importing `useMe` from the `queries.ts` barrel
 * shipped the whole module with it (see `json-fetch.ts`).
 */
import { useQuery } from "@tanstack/react-query";
import { usePathname } from "next/navigation";

import { jsonFetch } from "./json-fetch";
import { queryKeys } from "./query-keys";
import type { MeView } from "./types";

/** Routes that are anonymous by construction. */
const AUTH_SURFACE = /^\/(?:sign-in|forgot-password|reset-password)(?:\/|$)/;

export const isAuthSurface = (pathname: string | null) =>
  AUTH_SURFACE.test(pathname ?? "");

export function useMe({ enabled = true }: { enabled?: boolean } = {}) {
  // Never probe from the sign-in pages: there `/auth/me` only 401s, and the
  // 401 runs the refresh dance (`POST /auth/refresh`, a 403 without a
  // session) on every visit. Checked here rather than at each caller
  // because the root-layout listeners (GlobalHotkeys, GlobalShortcutsSheet,
  // ScanResultListener) sit in the reader's first-load bundle budget.
  // `enabled: false` still lets other callers skip the probe.
  const pathname = usePathname();
  return useQuery({
    queryKey: queryKeys.me,
    queryFn: () => jsonFetch<MeView>("/auth/me"),
    staleTime: 60_000,
    enabled: enabled && !isAuthSurface(pathname),
  });
}
