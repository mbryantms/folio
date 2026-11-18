"use client";

import * as React from "react";
import dynamic from "next/dynamic";
import {
  CirclePause,
  Pencil,
  CirclePlay,
  RefreshCw,
  Sparkles,
  TableProperties,
} from "lucide-react";

import {
  COVERAGE_SELECTION_MAX_SERIES,
  FETCH_SELECTION_MAX_ISSUES,
  FETCH_SELECTION_MAX_SERIES,
  RESCAN_SELECTION_MAX_ISSUES,
  RESCAN_SELECTION_MAX_SERIES,
  SYNC_SELECTION_MAX_SERIES,
  useBulkAnalyzeCoverage,
  useBulkMetadataFetch,
  useBulkRescan,
  useBulkSetMetadataSync,
} from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";

import type { SelectionAction } from "./SelectionToolbar";

const EditMetadataDialog = dynamic(
  () => import("./EditMetadataDialog").then((m) => m.EditMetadataDialog),
  { ssr: false },
);

export type AdminSelectionTargets = {
  issueIds: string[];
  seriesIds: string[];
};

/**
 * What a surface can select: issue-only surfaces get the issue actions
 * (Edit metadata…), series-only surfaces the series actions (Analyze
 * coverage, Pause / Resume metadata sync), and `mixed` (a collection) both.
 * Fetch metadata and Rescan take either kind and always show.
 */
export type AdminSelectionScope = "issues" | "series" | "mixed";

/**
 * The admin-only bulk actions every multi-select surface shares, for a
 * selection of issues and/or series. Readers get an empty list, so a
 * surface spreads `actions` into its Editing group and drops the group when
 * it comes back empty; it renders `dialogs` once (the lazy Edit metadata
 * dialog).
 *
 * `disabled` greys every action out (e.g. while "select all matching"
 * holds no concrete ids); an over-cap selection disables an action rather
 * than sending a request the server would refuse. `onDone` runs after a
 * successful request or when the edit dialog closes (callers clear or exit
 * the selection there).
 */
export function useAdminSelectionActions(
  targets: AdminSelectionTargets,
  opts: {
    scope: AdminSelectionScope;
    onDone?: () => void;
    disabled?: boolean;
  },
): {
  actions: SelectionAction[];
  dialogs: React.ReactNode;
  isPending: boolean;
} {
  const isAdmin = useMe().data?.role === "admin";
  const fetchMeta = useBulkMetadataFetch();
  const rescan = useBulkRescan();
  const coverage = useBulkAnalyzeCoverage();
  const sync = useBulkSetMetadataSync();
  const [editOpen, setEditOpen] = React.useState(false);
  // Mount the lazy dialog on first open and keep it mounted so its
  // open/close animation still runs on later toggles (G6).
  const [editMounted, setEditMounted] = React.useState(false);
  if (editOpen && !editMounted) setEditMounted(true);

  const isPending =
    fetchMeta.isPending ||
    rescan.isPending ||
    coverage.isPending ||
    sync.isPending;
  if (!isAdmin) return { actions: [], dialogs: null, isPending };

  const { issueIds, seriesIds } = targets;
  const { scope } = opts;
  const off = opts.disabled || isPending;
  const none = issueIds.length === 0 && seriesIds.length === 0;
  const noSeries = seriesIds.length === 0;
  const body = { issue_ids: issueIds, series_ids: seriesIds };
  const after = { onSuccess: () => opts.onDone?.() };
  const showIssueActions = scope !== "series";
  const showSeriesActions = scope !== "issues";

  const actions: SelectionAction[] = [];
  if (showIssueActions) {
    actions.push({
      id: "edit-metadata",
      label: "Edit metadata…",
      icon: Pencil,
      onClick: () => setEditOpen(true),
      disabled: off || issueIds.length === 0,
    });
  }
  actions.push(
    {
      id: "fetch-metadata",
      label: "Fetch metadata",
      icon: Sparkles,
      onClick: () => fetchMeta.mutate(body, after),
      disabled:
        off ||
        none ||
        issueIds.length > FETCH_SELECTION_MAX_ISSUES ||
        seriesIds.length > FETCH_SELECTION_MAX_SERIES,
    },
    {
      id: "rescan",
      label: "Rescan",
      icon: RefreshCw,
      onClick: () => rescan.mutate(body, after),
      disabled:
        off ||
        none ||
        issueIds.length > RESCAN_SELECTION_MAX_ISSUES ||
        seriesIds.length > RESCAN_SELECTION_MAX_SERIES,
    },
  );
  if (showSeriesActions) {
    const overSync = seriesIds.length > SYNC_SELECTION_MAX_SERIES;
    actions.push(
      {
        id: "analyze-coverage",
        label: "Analyze coverage",
        icon: TableProperties,
        onClick: () => coverage.mutate({ series_ids: seriesIds }, after),
        disabled:
          off || noSeries || seriesIds.length > COVERAGE_SELECTION_MAX_SERIES,
      },
      {
        id: "pause-sync",
        label: "Pause metadata sync",
        icon: CirclePause,
        onClick: () =>
          sync.mutate({ series_ids: seriesIds, paused: true }, after),
        disabled: off || noSeries || overSync,
      },
      {
        id: "resume-sync",
        label: "Resume metadata sync",
        icon: CirclePlay,
        onClick: () =>
          sync.mutate({ series_ids: seriesIds, paused: false }, after),
        disabled: off || noSeries || overSync,
      },
    );
  }

  const dialogs =
    showIssueActions && editMounted ? (
      <EditMetadataDialog
        open={editOpen}
        onOpenChange={(next) => {
          setEditOpen(next);
          if (!next) opts.onDone?.();
        }}
        issueIds={issueIds}
      />
    ) : null;

  return { actions, dialogs, isPending };
}
