"use client";

/**
 * Live unmatched-series count pill rendered next to the Metadata
 * sidebar entry. Backed by `useAdminMetadataDashboard` so it picks
 * up the same 60s refetch as the dashboard page itself — no
 * additional polling layered on the nav.
 *
 * Silent when the count is zero or the query hasn't resolved yet
 * (avoids a flash of "0" on first paint that an operator would
 * misread as "nothing to do").
 */

import { useAdminMetadataDashboard } from "@/lib/api/queries";

export function MetadataUnmatchedBadge({
  collapsed = false,
}: {
  collapsed?: boolean;
}) {
  const { data } = useAdminMetadataDashboard();
  const count = data?.series_unmatched ?? 0;
  if (!count) return null;
  const compact = count > 99 ? "99+" : String(count);
  if (collapsed) {
    return (
      <span
        aria-label={`${count} unmatched series`}
        className="bg-amber-500/15 text-amber-700 dark:text-amber-400 absolute -top-1 -right-1 inline-flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[9px] font-semibold leading-none"
      >
        {compact}
      </span>
    );
  }
  return (
    <span
      aria-label={`${count} unmatched series`}
      className="bg-amber-500/15 text-amber-700 dark:text-amber-400 ml-auto inline-flex h-4 min-w-4 items-center justify-center rounded-full px-1 text-[10px] font-semibold leading-none"
    >
      {compact}
    </span>
  );
}
