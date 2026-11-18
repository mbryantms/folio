"use client";

import { RefreshCw, Sparkles } from "lucide-react";

import {
  FETCH_SELECTION_MAX_ISSUES,
  FETCH_SELECTION_MAX_SERIES,
  RESCAN_SELECTION_MAX_ISSUES,
  RESCAN_SELECTION_MAX_SERIES,
  useBulkMetadataFetch,
  useBulkRescan,
} from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";

import type { SelectionAction } from "./SelectionToolbar";

export type AdminSelectionTargets = {
  issueIds: string[];
  seriesIds: string[];
};

/**
 * The admin-only bulk actions every multi-select surface shares —
 * "Fetch metadata" and "Rescan" — for a selection of issues and/or series.
 * Readers get an empty list, so a surface can spread the result into its
 * Editing group and drop the group when it comes back empty.
 *
 * `disabled` greys both out (e.g. while "select all matching" holds no
 * concrete ids); an over-cap selection disables an action rather than
 * sending a request the server would refuse. `onDone` runs after a
 * successful request (callers clear or exit the selection there).
 */
export function useAdminSelectionActions(
  targets: AdminSelectionTargets,
  opts: { onDone?: () => void; disabled?: boolean } = {},
): { actions: SelectionAction[]; isPending: boolean } {
  const isAdmin = useMe().data?.role === "admin";
  const fetchMeta = useBulkMetadataFetch();
  const rescan = useBulkRescan();
  const isPending = fetchMeta.isPending || rescan.isPending;
  if (!isAdmin) return { actions: [], isPending };

  const { issueIds, seriesIds } = targets;
  const empty = issueIds.length === 0 && seriesIds.length === 0;
  const body = { issue_ids: issueIds, series_ids: seriesIds };
  const after = { onSuccess: () => opts.onDone?.() };

  return {
    isPending,
    actions: [
      {
        id: "fetch-metadata",
        label: "Fetch metadata",
        icon: Sparkles,
        onClick: () => fetchMeta.mutate(body, after),
        disabled:
          opts.disabled ||
          empty ||
          isPending ||
          issueIds.length > FETCH_SELECTION_MAX_ISSUES ||
          seriesIds.length > FETCH_SELECTION_MAX_SERIES,
      },
      {
        id: "rescan",
        label: "Rescan",
        icon: RefreshCw,
        onClick: () => rescan.mutate(body, after),
        disabled:
          opts.disabled ||
          empty ||
          isPending ||
          issueIds.length > RESCAN_SELECTION_MAX_ISSUES ||
          seriesIds.length > RESCAN_SELECTION_MAX_SERIES,
      },
    ],
  };
}
