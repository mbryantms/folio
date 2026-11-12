"use client";

import * as React from "react";
import { PageHeader } from "@/components/admin/PageHeader";
import { ReadingListsBoard } from "@/components/cbl/ReadingListsBoard";
import { splitSavedViews } from "@/components/saved-views/ViewsIndex";
import { useSavedViews } from "@/lib/api/queries";

/**
 * Reader-side reading-lists index (`/lists`) — the sidebar "Reading lists"
 * entry lands here. The caller's own imported CBL lists and the lists an
 * admin shared with everyone, as a tree table of folders and lists, each
 * row opening the list's detail view.
 * The board is the same one Settings → Views shows on its Reading lists
 * tab (`ReadingListsBoard`), so import, updates and grouping behave the
 * same in both places.
 *
 * `canImport` is a prop rather than a role check: every signed-in account
 * may import (the server bounds it with a per-user hourly quota), so the
 * route passes `true`.
 */
export function ReadingListsIndex({
  canImport = true,
}: {
  canImport?: boolean;
}) {
  const viewsQ = useSavedViews();
  // Other surfaces share the saved-views query, so the cache can already
  // hold it when this route's segment hydrates; render the server's
  // (loading) markup until hydrated, or React reports a mismatch.
  const hydrated = React.useSyncExternalStore(
    noopSubscribe,
    () => true,
    () => false,
  );
  const readingLists = React.useMemo(
    () =>
      hydrated ? splitSavedViews(viewsQ.data?.items ?? []).readingLists : [],
    [hydrated, viewsQ.data],
  );

  return (
    <div className="space-y-6">
      <PageHeader
        title="Reading lists"
        description="Imported reading orders: your own and the ones shared with everyone. Open a list to read it in order or resolve unmatched entries."
      />
      {viewsQ.isError ? (
        <div className="text-destructive rounded-md border p-4 text-sm">
          Failed to load reading lists.
        </div>
      ) : (
        <ReadingListsBoard
          views={readingLists}
          loading={!hydrated || viewsQ.isLoading}
          canImport={canImport}
        />
      )}
    </div>
  );
}

const noopSubscribe = () => () => {};
