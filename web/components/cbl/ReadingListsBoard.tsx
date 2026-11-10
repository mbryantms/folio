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
import { GroupedViewsBoard } from "@/components/saved-views/GroupedViewsBoard";
import { useCblLists } from "@/lib/api/queries";
import type { CblListView, SavedViewView } from "@/lib/api/types";
import { BatchRefreshProvider } from "@/lib/cbl/use-batch-refresh";
import type { BoardGroup } from "@/lib/saved-views/view-board";

/** CBL list rows keyed by list id, so each reading-list card can show
 *  stats without its own detail query. `null` until loaded. */
export const CblListsContext = React.createContext<Map<
  string,
  CblListView
> | null>(null);

/**
 * The reading-list board: the caller's own CBL views plus the ones shared
 * with everyone, as a grouped board with the updates feature in its
 * toolbar and an Import CBL action. One body for both homes of the list:
 * the Reading lists tab of Settings → Views and the reader-side `/lists`
 * index. `canImport` drops the import button and dialog for a caller who
 * may not import.
 */
export function ReadingListsBoard({
  views,
  loading,
  renderCard,
  canImport = true,
}: {
  views: SavedViewView[];
  loading: boolean;
  renderCard: (view: SavedViewView) => React.ReactNode;
  canImport?: boolean;
}) {
  const cblListsQ = useCblLists();
  const cblListsById = React.useMemo(
    () =>
      cblListsQ.data
        ? new Map(cblListsQ.data.items.map((l) => [l.id, l]))
        : null,
    [cblListsQ.data],
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
            renderCard={renderCard}
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
  renderCard,
}: {
  views: SavedViewView[];
  loading: boolean;
  onImport: (() => void) | null;
  renderCard: (view: SavedViewView) => React.ReactNode;
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
      blurb="Track an imported CBL reading order."
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
      renderCard={renderCard}
      groupMenuExtra={groupMenuExtra}
      empty={
        onImport
          ? "No reading lists yet — import a CBL to track a reading order."
          : "No reading lists yet."
      }
    />
  );
}
