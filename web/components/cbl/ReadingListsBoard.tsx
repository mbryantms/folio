"use client";

import * as React from "react";
import { List, Plus, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { DropdownMenuItem } from "@/components/folio/dropdown-menu";
import { CblImportDialog } from "@/components/cbl/cbl-import-dialog";
import {
  CblUpdatesMenu,
  CblUpdatesPanel,
  CblUpdatesProvider,
  useCblUpdates,
} from "@/components/cbl/CblRefreshDashboard";
import { CblListsContext } from "@/components/cbl/cbl-lists-context";
import { readingListRows } from "@/components/cbl/ReadingListRows";
import {
  GroupedViewsBoard,
  type BoardRowsPresentation,
} from "@/components/saved-views/GroupedViewsBoard";
import { useCblLists } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";
import { BatchRefreshProvider } from "@/lib/cbl/use-batch-refresh";
import type { BoardGroup } from "@/lib/saved-views/view-board";

export { CblListsContext };

/**
 * The reading-list board: the caller's own CBL views plus the ones shared
 * with everyone, as a grouped board with the updates feature in its
 * toolbar and an Import CBL action. One body for both homes of the list:
 * the Reading lists tab of Settings → Views and the reader-side `/lists`
 * index. Lists render as the board's tree table (`readingListRows`):
 * folders with roll-ups, a row per list. `canImport` drops the import
 * button and dialog for a caller who may not import.
 *
 * `/me/cbl-lists` and `/me/saved-views` both return every row (no cursor,
 * no cap), so the roll-ups and the "Most read" sort work on complete data.
 */
export function ReadingListsBoard({
  views,
  loading,
  canImport = true,
  blurb = "Track an imported CBL reading order.",
}: {
  views: SavedViewView[];
  loading: boolean;
  canImport?: boolean;
  /** The toolbar's line; `null` where the page header says it already. */
  blurb?: string | null;
}) {
  const cblListsQ = useCblLists();
  const cblListsById = React.useMemo(
    () =>
      cblListsQ.data
        ? new Map(cblListsQ.data.items.map((l) => [l.id, l]))
        : null,
    [cblListsQ.data],
  );
  const rows = React.useMemo(
    () => readingListRows(cblListsById),
    [cblListsById],
  );
  const [importOpen, setImportOpen] = React.useState(false);
  const targets = React.useMemo(
    () =>
      views.flatMap((v) =>
        v.cbl_list_id
          ? [{ id: v.cbl_list_id, name: v.name, viewId: v.id }]
          : [],
      ),
    [views],
  );
  return (
    <CblListsContext.Provider value={cblListsById}>
      <BatchRefreshProvider>
        <CblUpdatesProvider lists={targets}>
          <Board
            views={views}
            loading={loading}
            onImport={canImport ? () => setImportOpen(true) : null}
            rows={rows}
            blurb={blurb}
          />
        </CblUpdatesProvider>
      </BatchRefreshProvider>
      {canImport ? (
        <CblImportDialog open={importOpen} onOpenChange={setImportOpen} />
      ) : null}
    </CblListsContext.Provider>
  );
}

/** The grouped board with the updates feature tucked into its toolbar.
 *  The "Updates" menu runs a check or a bulk update and reveals the
 *  updates panel under the toolbar; the panel stays hidden otherwise.
 *  Each group's ⋯ menu gets "Update these lists", which feeds the same
 *  runner and opens the same panel. */
function Board({
  views,
  loading,
  onImport,
  rows,
  blurb,
}: {
  views: SavedViewView[];
  loading: boolean;
  onImport: (() => void) | null;
  rows: BoardRowsPresentation;
  blurb: string | null;
}) {
  const { busy, startBatch } = useCblUpdates();
  const groupMenuExtra = React.useCallback(
    (group: BoardGroup) => {
      const ids = group.views.flatMap((v) =>
        v.cbl_list_id ? [{ id: v.cbl_list_id, name: v.name }] : [],
      );
      return (
        <DropdownMenuItem
          disabled={ids.length === 0 || busy}
          onSelect={() => startBatch(ids)}
        >
          <RefreshCw className="size-4" /> Update{" "}
          {ids.length === 1 ? "this list" : `these ${ids.length} lists`}
        </DropdownMenuItem>
      );
    },
    [busy, startBatch],
  );
  return (
    <GroupedViewsBoard
      scope="cbl"
      views={views}
      loading={loading}
      blurb={blurb}
      noun="list"
      icon={List}
      action={
        <>
          <CblUpdatesMenu />
          {onImport ? (
            <Button type="button" onClick={onImport}>
              <Plus className="mr-1 size-4" /> Import CBL
            </Button>
          ) : null}
        </>
      }
      panel={<CblUpdatesPanel />}
      rows={rows}
      groupMenuExtra={groupMenuExtra}
      empty={
        onImport
          ? "No reading lists yet — import a CBL to track a reading order."
          : "No reading lists yet."
      }
    />
  );
}
