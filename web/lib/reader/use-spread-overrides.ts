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
 */
import { useMemo } from "react";

import { useIssuePageOverrides } from "@/lib/api/queries";
import type { PageOverridesView } from "@/lib/api/types";
import type { SpreadOverrides } from "@/lib/reader/spreads";

export function useSpreadOverrides(
  issueId: string,
  initial?: PageOverridesView | null,
): SpreadOverrides | null {
  const { data } = useIssuePageOverrides(issueId, initial);
  return useMemo(
    () =>
      data
        ? {
            shift_pairing: data.shift_pairing,
            spread_pages: data.spread_pages,
            single_pages: data.single_pages,
          }
        : null,
    [data],
  );
}
