/**
 * Library-wide sidecar re-emit (audit 2026-11 A7). Re-exported from
 * `mutations/index.ts`, so callers import from `"@/lib/api/mutations"`.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type { SidecarReemitRunView } from "../types";
import { useApiMutation } from "./_core";

/**
 * `POST /libraries/{slug}/sidecar-reemit`. Starts a background run that
 * rewrites the ComicInfo / MetronInfo files of every issue in the library
 * from the database. The server refuses (409) unless both writeback flags
 * are on, or while a run is already live.
 */
export function useStartSidecarReemit(librarySlug: string) {
  const qc = useQueryClient();
  return useApiMutation<SidecarReemitRunView, void>(
    () => ({
      path: `/libraries/${librarySlug}/sidecar-reemit`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        `Rewriting metadata files for ${(data?.total ?? 0).toLocaleString("en-US")} issue${
          data?.total === 1 ? "" : "s"
        } in the background`,
      onSuccess: () => {
        qc.invalidateQueries({
          queryKey: queryKeys.sidecarReemit(librarySlug),
        });
        qc.invalidateQueries({ queryKey: queryKeys.backgroundWork });
        qc.invalidateQueries({ queryKey: queryKeys.queueDepth });
      },
    },
  );
}
