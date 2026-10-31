"use client";

/**
 * WP-4.3 — the current user's manual spread controls for one issue, as
 * the `SpreadOverrides` shape `computeSpreadGroups` consumes.
 *
 * Backed by one TanStack query (`queryKeys.issuePageOverrides`), so the
 * reader, the page strip and the settings popover all read — and the
 * mutations in `@/lib/api/mutations/page-overrides` all write — the
 * same cache entry. Returns `null` while nothing is known (first load
 * without an SSR prefetch, or a failed fetch): automatic pairing is the
 * safe fallback.
 *
 * `stored` is the device's copy from the issue's download record (the
 * offline reader): it stands in while the fetch is pending or failed,
 * and the live answer wins when it arrives — so an offline open pairs
 * the pages as it did online instead of re-pairing automatically
 * (audit N-R7).
 */
import { useMemo } from "react";

import { useIssuePageOverrides } from "@/lib/api/queries";
import type { PageOverridesView } from "@/lib/api/types";
import type { SpreadOverrides } from "@/lib/reader/spreads";

export function useSpreadOverrides(
  issueId: string,
  initial?: PageOverridesView | null,
  stored?: PageOverridesView | null,
): SpreadOverrides | null {
  const { data } = useIssuePageOverrides(issueId, initial);
  const view = data ?? stored ?? null;
  return useMemo(
    () =>
      view
        ? {
            shift_pairing: view.shift_pairing,
            spread_pages: view.spread_pages,
            single_pages: view.single_pages,
          }
        : null,
    [view],
  );
}
