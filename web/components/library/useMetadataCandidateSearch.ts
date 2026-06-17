"use client";

/**
 * Provider-search orchestration for the metadata-match dialog
 * (metadata-providers-1.0 M5; extracted in H4 Phase 3).
 *
 * Owns the run lifecycle the dialog drives its candidate list off:
 *   - `runId` — the active provider-search run (null until created)
 *   - `searchPending` / `searchError` / `reused` — surface state
 *   - the one-shot auto-kick on open (probe-or-search, quota saver)
 *   - `researchFromScratch()` — the explicit "Re-search" force path
 *
 * **Why raw `apiMutate`, not `useApiMutation`:** the search is kicked from
 * an effect, and TanStack Query v5's mutation observer ends up disconnected
 * from its state machine when fired from an effect that React 19 StrictMode
 * tears down in its intentional dev mount → unmount → remount cycle. The
 * mutationFn promise resolves (the network roundtrip completes), but the
 * observer's `data`/`status`/per-call `onSuccess` all silently drop on the
 * floor. Bypassing the observer with a direct `apiMutate` call lands the
 * response in plain React state (`searchPending`/`runId`), which survives the
 * strict-mode cycle like any other `useState`. Do not "modernize" this to
 * `useApiMutation` without re-verifying the StrictMode auto-kick path.
 *
 * The hook deliberately owns ONLY the run/search slice. The dialog keeps the
 * candidate query (which reads `runId`), the apply/preview/compare state, and
 * the per-ordinal seeding effects — `researchFromScratch` resets the run + the
 * auto-kick guard, and the dialog resets its own form state around the call.
 */

import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { toast } from "sonner";

import { ApiMutationError, apiMutate } from "@/lib/api/mutations";
import { jsonFetch } from "@/lib/api/queries";
import type { MetadataMatchScope } from "@/components/library/metadata-match-scope";
import type { CandidatesResp, SearchStartedResp } from "@/lib/api/types";

export interface MetadataCandidateSearch {
  /** Run id of the active provider search; null until one is created. */
  runId: string | null;
  /** The search-creation POST is in flight. */
  searchPending: boolean;
  /** Human-readable search failure, or null. */
  searchError: string | null;
  /** Machine-readable `error.code` of the search failure (e.g.
   *  `"metadata.no_providers"`), or null. Lets the dialog show a
   *  pre-flight "configure a provider" notice instead of a bare retry. */
  searchErrorCode: string | null;
  /** True when the dialog adopted a prior completed run (quota saver)
   *  instead of firing a fresh provider fan-out. */
  reused: boolean;
  /** Force a fresh provider search from scratch — clears the run + the
   *  one-shot auto-kick guard, then fires `runSearch`. The caller is
   *  responsible for resetting its own form state (compare/preview/seed)
   *  around this call. */
  researchFromScratch: () => void;
}

export function useMetadataCandidateSearch({
  scope,
  open,
}: {
  scope: MetadataMatchScope;
  open: boolean;
}): MetadataCandidateSearch {
  const qc = useQueryClient();
  const [runId, setRunId] = React.useState<string | null>(null);
  const [searchPending, setSearchPending] = React.useState(false);
  const [searchError, setSearchError] = React.useState<string | null>(null);
  const [searchErrorCode, setSearchErrorCode] = React.useState<string | null>(
    null,
  );
  // True when the dialog adopted a prior completed run instead of
  // firing a fresh provider search (quota saver).
  const [reused, setReused] = React.useState(false);

  const searchPath = React.useMemo(
    () =>
      scope.kind === "series"
        ? `/series/${encodeURIComponent(scope.seriesSlug)}/metadata/search`
        : `/series/${encodeURIComponent(scope.seriesSlug)}/issues/${encodeURIComponent(scope.issueSlug)}/metadata/search`,
    [scope],
  );
  const candidatesInvalidateKey = React.useMemo(
    () =>
      scope.kind === "series"
        ? ["series", scope.seriesSlug, "metadata", "candidates"]
        : [
            "series",
            scope.seriesSlug,
            "issues",
            scope.issueSlug,
            "metadata",
            "candidates",
          ],
    [scope],
  );

  const candidatesProbePath = React.useMemo(
    () =>
      scope.kind === "series"
        ? `/series/${encodeURIComponent(scope.seriesSlug)}/metadata/candidates`
        : `/series/${encodeURIComponent(scope.seriesSlug)}/issues/${encodeURIComponent(scope.issueSlug)}/metadata/candidates`,
    [scope],
  );

  const runSearch = React.useCallback(() => {
    setSearchPending(true);
    setSearchError(null);
    setSearchErrorCode(null);
    setReused(false);
    let cancelled = false;
    void (async () => {
      try {
        const result = await apiMutate<SearchStartedResp>({
          path: searchPath,
          method: "POST",
        });
        if (cancelled) return;
        if (result?.run_id) {
          setRunId(result.run_id);
          qc.invalidateQueries({ queryKey: candidatesInvalidateKey });
        } else {
          setSearchError("Empty response from search endpoint.");
        }
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : "Search failed.";
        setSearchError(msg);
        setSearchErrorCode(e instanceof ApiMutationError ? e.code : null);
        toast.error(msg);
      } finally {
        if (!cancelled) setSearchPending(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [searchPath, candidatesInvalidateKey, qc]);

  // Quota saver: on open, probe the latest completed run for this scope
  // (a cheap GET, no provider quota). If one exists with candidates,
  // adopt it instead of firing a fresh fan-out. The explicit "Re-search"
  // button always forces a fresh search via `runSearch`.
  const kickOrReuse = React.useCallback(() => {
    setSearchPending(true);
    setSearchError(null);
    setSearchErrorCode(null);
    setReused(false);
    let cancelled = false;
    void (async () => {
      try {
        const latest = await jsonFetch<CandidatesResp>(
          candidatesProbePath,
        ).catch(() => null);
        if (cancelled) return;
        if (
          latest &&
          latest.status === "completed" &&
          (latest.candidates?.length ?? 0) > 0 &&
          latest.run_id
        ) {
          setRunId(latest.run_id);
          setReused(true);
          setSearchPending(false);
          return;
        }
        const result = await apiMutate<SearchStartedResp>({
          path: searchPath,
          method: "POST",
        });
        if (cancelled) return;
        if (result?.run_id) {
          setRunId(result.run_id);
          qc.invalidateQueries({ queryKey: candidatesInvalidateKey });
        } else {
          setSearchError("Empty response from search endpoint.");
        }
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : "Search failed.";
        setSearchError(msg);
        setSearchErrorCode(e instanceof ApiMutationError ? e.code : null);
        toast.error(msg);
      } finally {
        if (!cancelled) setSearchPending(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [searchPath, candidatesProbePath, candidatesInvalidateKey, qc]);

  const kickedRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      kickedRef.current = false;
      return;
    }
    if (kickedRef.current) return;
    kickedRef.current = true;
    kickOrReuse();
    // kickOrReuse is stable per (searchPath, probePath, key) — those
    // don't change mid-dialog.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const researchFromScratch = React.useCallback(() => {
    setRunId(null);
    kickedRef.current = false;
    runSearch();
  }, [runSearch]);

  return {
    runId,
    searchPending,
    searchError,
    searchErrorCode,
    reused,
    researchFromScratch,
  };
}
