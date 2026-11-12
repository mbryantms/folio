"use client";

import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  Check,
  FolderInput,
  FolderPlus,
  MoreVertical,
  Pin,
  Trash2,
  type LucideIcon,
  PanelLeft,
  PanelLeftClose,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { DropdownMenuResponsiveSub } from "@/components/folio/dropdown-menu-responsive-sub";
import { useViewBoardActions } from "@/components/saved-views/GroupedViewsBoard";
import { MultiPinDialog } from "@/components/saved-views/MultiPinDialog";
import { useDeleteCollection, useSidebarSavedView } from "@/lib/api/mutations";
import { apiMutate, useApiMutation } from "@/lib/api/mutations/_core";
import { queryKeys } from "@/lib/api/queries";
import type {
  Condition,
  CreateSavedViewReq,
  MatchMode,
  SavedViewView,
  SortOrder,
} from "@/lib/api/types";
import {
  fetchCollectionSnapshot,
  type CollectionSnapshot,
} from "@/lib/collections/recreate";
import { useCollectionDeleteUndo } from "@/lib/collections/use-collection-undo";
import { UNGROUPED_ID } from "@/lib/saved-views/view-board";
import { showUndoToast } from "@/lib/ui/undo-toast";
import { cn } from "@/lib/utils";

export const WANT_TO_READ_KEY = "want_to_read";

/** Where a saved-content card or row opens. */
export function viewHref(view: SavedViewView, isCollection: boolean): string {
  const isWantToRead = isCollection && view.system_key === WANT_TO_READ_KEY;
  return isCollection
    ? `/views/${isWantToRead ? "want-to-read" : view.id}`
    : `/views/${view.id}`;
}

/** True for rows the user cannot delete (system rails, shared lists,
 *  Want to Read). */
export function isBuiltInView(
  view: SavedViewView,
  isCollection: boolean,
): boolean {
  return (
    view.is_system || (isCollection && view.system_key === WANT_TO_READ_KEY)
  );
}

/** A saved view's ⋯ menu, shared by the card and the reading-lists row:
 *  pin-to-pages (the search + collapsible `MultiPinDialog`),
 *  show-in-sidebar, Move to group (the keyboard / touch path for what
 *  drag-and-drop does, when inside a board), and Delete with Undo.
 *  Built-in rows (system rails + Want to Read) lock Delete. */
export function SavedViewMenu({
  view,
  isCollection,
  triggerClassName,
  icon: TriggerIcon = MoreVertical,
}: {
  view: SavedViewView;
  isCollection: boolean;
  triggerClassName?: string;
  icon?: LucideIcon;
}) {
  const qc = useQueryClient();
  // Undo instead of confirm (folio-flair-1.0): Delete fires at once and
  // the toast carries an Undo that rebuilds the view from a snapshot.
  // Both delete mutations are silent — the Undo toast is the only one.
  const delSaved = useApiMutation<unknown, void>(
    () => ({ path: `/me/saved-views/${view.id}`, method: "DELETE" }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"] });
        // A deleted view's sidebar row would otherwise linger until reload.
        qc.invalidateQueries({ queryKey: queryKeys.sidebarLayout });
      },
    },
  );
  const delCollection = useDeleteCollection(view.id, { silent: true });
  const showDeleteUndo = useCollectionDeleteUndo();
  const sidebar = useSidebarSavedView();
  const board = useViewBoardActions();
  const [pinOpen, setPinOpen] = React.useState(false);
  const [deleting, setDeleting] = React.useState(false);

  const isBuiltIn = isBuiltInView(view, isCollection);
  const pinnedCount = (view.pinned_on_pages ?? []).length;
  const currentGroup = board?.containerOf(view.id) ?? null;

  async function handleDelete() {
    if (deleting) return;
    if (!isCollection) {
      // Filter / CBL views: the view row is all that goes (a CBL view's
      // underlying list stays), so Undo re-creates it from its definition.
      const snapshot = view;
      delSaved.mutate(undefined, {
        onSuccess: () =>
          showUndoToast({
            message: `Deleted "${snapshot.name}"`,
            restoredMessage: `Restored "${snapshot.name}"`,
            failedMessage: "Couldn't restore the view",
            onUndo: async () => {
              const created = await recreateSavedView(snapshot);
              if (!created) throw new Error("recreate returned null");
              qc.invalidateQueries({ queryKey: ["saved-views"] });
              qc.invalidateQueries({ queryKey: queryKeys.sidebarLayout });
            },
          }),
      });
      return;
    }
    // Collections: snapshot the full member list *before* the delete drops
    // it, so the Undo toast can replay it. A snapshot failure (offline /
    // race) still deletes — just without the undo affordance.
    setDeleting(true);
    let snapshot: CollectionSnapshot | null = null;
    try {
      snapshot = await fetchCollectionSnapshot(
        view.id,
        view.name,
        view.description,
      );
    } catch {
      snapshot = null;
    }
    delCollection.mutate(undefined, {
      onSuccess: () => {
        if (snapshot) showDeleteUndo(snapshot);
        else toast.success(`Collection "${view.name}" deleted`);
      },
      onSettled: () => setDeleting(false),
    });
  }

  function toggleSidebar(next: boolean) {
    sidebar.mutate(
      { id: view.id, show: next },
      // The sidebar mutation optimistically patches the saved-views cache;
      // collections read from a separate cache, so refresh it on settle.
      isCollection
        ? {
            onSettled: () =>
              qc.invalidateQueries({ queryKey: queryKeys.collections }),
          }
        : undefined,
    );
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={`Actions for ${view.name}`}
            className={cn(
              "text-muted-foreground hover:text-foreground",
              triggerClassName,
            )}
          >
            <TriggerIcon className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-56">
          {/* Same wording as the list's own page (`ViewActionsMenu`), so
              the menu also says where the view is pinned. */}
          <DropdownMenuItem onSelect={() => setPinOpen(true)}>
            <Pin className="size-4" />
            {pinnedCount === 0
              ? "Pin to pages…"
              : pinnedCount === 1
                ? "Pinned to 1 page"
                : `Pinned to ${pinnedCount} pages`}
          </DropdownMenuItem>
          <DropdownMenuItem
            onSelect={() => toggleSidebar(!view.show_in_sidebar)}
          >
            {view.show_in_sidebar ? (
              <>
                <PanelLeftClose className="size-4" /> Hide from sidebar
              </>
            ) : (
              <>
                <PanelLeft className="size-4" /> Show in sidebar
              </>
            )}
          </DropdownMenuItem>
          {board ? (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuResponsiveSub
                icon={<FolderInput className="size-4" />}
                label="Move to group"
              >
                {board.groups.map((g) => (
                  <DropdownMenuItem
                    key={g.id}
                    disabled={currentGroup === g.id}
                    onSelect={() => board.moveToGroup(view.id, g.id)}
                  >
                    <span className="inline-flex w-4 justify-center">
                      {currentGroup === g.id ? (
                        <Check className="size-4" aria-hidden="true" />
                      ) : null}
                    </span>
                    <span className="truncate">{g.name}</span>
                  </DropdownMenuItem>
                ))}
                {board.groups.length > 0 ? (
                  <DropdownMenuItem
                    disabled={currentGroup === UNGROUPED_ID}
                    onSelect={() => board.moveToGroup(view.id, null)}
                  >
                    <span className="inline-flex w-4 justify-center">
                      {currentGroup === UNGROUPED_ID ? (
                        <Check className="size-4" aria-hidden="true" />
                      ) : null}
                    </span>
                    Ungrouped
                  </DropdownMenuItem>
                ) : null}
                {board.groups.length > 0 ? <DropdownMenuSeparator /> : null}
                <DropdownMenuItem
                  onSelect={() => board.newGroupWithView(view.id)}
                >
                  <FolderPlus className="size-4" /> New group…
                </DropdownMenuItem>
              </DropdownMenuResponsiveSub>
            </>
          ) : null}
          {isBuiltIn ? null : (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                onSelect={() => void handleDelete()}
                disabled={deleting}
                className="text-destructive focus:text-destructive"
              >
                <Trash2 className="size-4" /> Delete
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <MultiPinDialog view={view} open={pinOpen} onOpenChange={setPinOpen} />
    </>
  );
}

/** Re-create a deleted filter / CBL saved view from its last-known row
 *  (the Undo half of the views-index Delete). Like collection undo
 *  (`lib/collections/recreate.ts`) it restores the *definition*, not the
 *  row: the new view gets a fresh id, so pins, sidebar placement, group
 *  membership and the icon override are not carried over. A CBL view
 *  points back at the same `cbl_list_id`, which a plain view delete
 *  leaves in place. Exported for tests. */
export async function recreateSavedView(
  view: SavedViewView,
): Promise<SavedViewView | null> {
  const body: CreateSavedViewReq = {
    kind: view.kind,
    name: view.name,
    description: view.description ?? null,
    custom_tags: view.custom_tags,
    custom_year_start: view.custom_year_start ?? null,
    custom_year_end: view.custom_year_end ?? null,
  };
  if (view.kind === "cbl") {
    body.cbl_list_id = view.cbl_list_id ?? null;
  } else {
    body.filter = {
      match_mode: (view.match_mode ?? "all") as MatchMode,
      conditions: (view.conditions ?? []) as Condition[],
    };
    body.sort_field = (view.sort_field ??
      null) as CreateSavedViewReq["sort_field"];
    body.sort_order = (view.sort_order ?? null) as SortOrder | null;
    body.result_limit = view.result_limit ?? null;
  }
  return apiMutate<SavedViewView>({
    path: "/me/saved-views",
    method: "POST",
    body,
  });
}
