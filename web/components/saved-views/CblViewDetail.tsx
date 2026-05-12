"use client";

import * as React from "react";
import { Download, RefreshCw } from "lucide-react";

import { CblDetail, CblInfoRow } from "@/components/cbl/cbl-detail";
import { CblIssueCard } from "@/components/cbl/cbl-issue-card";
import { CblStatsPills } from "@/components/cbl/CblStatsPills";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { useCardSize } from "@/components/library/use-card-size";
import { Button } from "@/components/ui/button";
import { PopoverPortalContainer } from "@/components/ui/popover";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { useCblList, useCblListIssues } from "@/lib/api/queries";
import { useRefreshCblList } from "@/lib/api/mutations";
import type { SavedViewView } from "@/lib/api/types";

import { ViewHeader } from "./ViewHeader";

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.savedView.cardSize";

/** Read-first detail page for a CBL saved view. Mirrors filter views:
 *  the page is a consumption surface (issues in CBL position order),
 *  with Edit / Pin/Unpin / Refresh / Export in the header. The full
 *  management UI (Reading order / Resolution / History / Settings
 *  tabs) lives behind the Edit button via a wide dialog. */
export function CblViewDetail({ savedView }: { savedView: SavedViewView }) {
  const listId = savedView.cbl_list_id;
  if (!listId) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Saved view is marked as CBL but has no `cbl_list_id`.
      </div>
    );
  }
  return <CblViewDetailInner savedView={savedView} listId={listId} />;
}

function CblViewDetailInner({
  savedView,
  listId,
}: {
  savedView: SavedViewView;
  listId: string;
}) {
  const detail = useCblList(listId);
  const issues = useCblListIssues(listId, { limit: 1000 });
  const refresh = useRefreshCblList(listId);
  const [editOpen, setEditOpen] = React.useState(false);
  // Re-anchors `ManualMatchPopover` (and any other descendant popover)
  // into the SheetContent subtree. Without this they portal to
  // document.body, where the Sheet's modal aria-hide makes the search
  // input render but reject focus/clicks.
  const [editPortalContainer, setEditPortalContainer] =
    React.useState<HTMLElement | null>(null);
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  if (detail.isLoading) {
    return (
      <div className="text-muted-foreground py-12 text-sm">Loading view…</div>
    );
  }
  if (detail.isError || !detail.data) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Failed to load view.
      </div>
    );
  }

  const list = detail.data;
  const entries = list.entries ?? [];
  const matchedIssues = issues.data?.items ?? [];
  // Walk both lists (entries is full, matchedIssues is in CBL position
  // order but excludes unmatched) to associate each entry with its
  // hydrated issue summary, falling back to the raw entry for
  // unmatched/ambiguous/missing rows.
  const issueByPosition = new Map<number, (typeof matchedIssues)[number]>();
  let cursor = 0;
  for (const entry of entries) {
    if (entry.matched_issue_id && cursor < matchedIssues.length) {
      issueByPosition.set(entry.position, matchedIssues[cursor]);
      cursor++;
    }
  }
  const canRefresh = list.source_kind !== "upload";
  const gridStyle: React.CSSProperties = {
    gridTemplateColumns: `repeat(auto-fill, minmax(${cardSize}px, 1fr))`,
  };

  return (
    <div className="space-y-6">
      <ViewHeader
        view={savedView}
        onEdit={() => setEditOpen(true)}
        extraActions={
          <>
            {/* Same two-pill summary the home rail header carries —
             *  `size="header"` bumps padding/typography so the pills
             *  line up with the adjacent `size="sm"` buttons. */}
            <CblStatsPills cblListId={list.id} size="header" />
            <CardSizeOptions
              cardSize={cardSize}
              onCardSize={setCardSize}
              min={CARD_SIZE_MIN}
              max={CARD_SIZE_MAX}
              step={CARD_SIZE_STEP}
              defaultSize={CARD_SIZE_DEFAULT}
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              asChild
              title="Download as .cbl"
            >
              <a href={`/api/me/cbl-lists/${list.id}/export`} download>
                <Download className="mr-1 h-4 w-4" /> Export
              </a>
            </Button>
            {canRefresh ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => refresh.mutate({})}
                disabled={refresh.isPending}
                title="Pull latest from source"
              >
                <RefreshCw
                  className={`mr-1 h-4 w-4 ${refresh.isPending ? "animate-spin" : ""}`}
                />
                Refresh
              </Button>
            ) : null}
          </>
        }
      />
      <CblInfoRow list={list} />
      {entries.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
          This list has no entries yet.
        </div>
      ) : (
        <ul role="list" className="grid gap-3" style={gridStyle}>
          {entries.map((entry) => (
            <li key={entry.id}>
              <CblIssueCard
                entry={entry}
                issue={issueByPosition.get(entry.position)}
              />
            </li>
          ))}
        </ul>
      )}

      <Sheet open={editOpen} onOpenChange={setEditOpen}>
        <SheetContent
          ref={setEditPortalContainer}
          side="right"
          // Wider than the filter-view sheet — the Reading-order tab
          // hosts a 6-column virtualized table that wants more
          // horizontal room than the filter builder does. `p-0` so
          // the header gets its own divider and the body owns its
          // padding. `overflow-visible` so the manual-match popover
          // (portaled into this content via PopoverPortalContainer)
          // can extend past the sheet's edge when collision detection
          // flips it outward; tab bodies own their own scroll.
          className="flex w-full flex-col gap-0 overflow-visible p-0 sm:max-w-3xl lg:max-w-4xl xl:max-w-5xl"
        >
          <SheetHeader className="border-border/60 border-b px-6 py-4 pr-12">
            <SheetTitle>Manage {savedView.name}</SheetTitle>
            <SheetDescription>
              Resolve missing or ambiguous matches, review refresh history,
              tweak metadata.
            </SheetDescription>
          </SheetHeader>
          <PopoverPortalContainer value={editPortalContainer}>
            <div className="flex min-h-0 flex-1 flex-col px-6 py-4">
              <CblDetail savedView={savedView} />
            </div>
          </PopoverPortalContainer>
        </SheetContent>
      </Sheet>
    </div>
  );
}
