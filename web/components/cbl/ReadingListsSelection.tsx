"use client";

/**
 * Select mode for the reading-lists board: tick lists (or a whole group
 * from its menu), then delete them together.
 *
 * Deleting a reading list's view deletes the list itself when the reader
 * owns it (`DELETE /me/saved-views/{id}`), so this asks first and offers
 * no Undo. Lists are deleted one at a time, so the dialog can show how
 * far it is and a failure leaves the rest of the selection in place to
 * retry. Lists shared with everyone are not the reader's to delete here
 * and cannot be ticked.
 */
import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ListChecks, Trash2, X } from "lucide-react";
import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { apiMutate } from "@/lib/api/mutations/_core";
import { queryKeys } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

/** A list the reader may delete: their own, not one shared with everyone. */
export function canDeleteList(view: SavedViewView): boolean {
  return view.kind === "cbl" && !view.is_system;
}

/**
 * Whether deleting this view also deletes its reading list: the server
 * does that when the reader owns the list (`saved_views::delete_one`).
 * Their own view of a list someone else shares is only the view row, and
 * that one can be re-created. A list not loaded yet counts as owned, so
 * the caller asks first instead of offering an Undo that cannot work.
 */
export function deleteTakesList(
  view: SavedViewView,
  lists: ReadonlyMap<string, { owner_user_id?: string | null }> | null,
  meId: string | null | undefined,
): boolean {
  if (view.kind !== "cbl" || !view.cbl_list_id) return false;
  const list = lists?.get(view.cbl_list_id);
  if (!list || !meId) return true;
  return list.owner_user_id === meId;
}

/** "Delete 3 reading lists?" / "Delete "Infinity"?" */
export function bulkDeleteTitle(views: readonly SavedViewView[]): string {
  return views.length === 1
    ? `Delete "${views[0]!.name}"?`
    : `Delete ${views.length} reading lists?`;
}

/** The toast after a run: what went and what stayed. */
export function bulkDeleteResult(
  deleted: number,
  failed: number,
): { ok: boolean; message: string } {
  const lists = (n: number) => `${n} reading ${n === 1 ? "list" : "lists"}`;
  if (failed === 0) return { ok: true, message: `Deleted ${lists(deleted)}` };
  if (deleted === 0)
    return { ok: false, message: `Couldn't delete ${lists(failed)}` };
  return {
    ok: false,
    message: `Deleted ${lists(deleted)}; ${failed} could not be deleted`,
  };
}

type Selection = {
  selecting: boolean;
  selected: ReadonlySet<string>;
  /** Enter select mode, optionally with these views ticked. */
  begin: (viewIds?: readonly string[]) => void;
  toggle: (viewId: string) => void;
  setAll: (viewIds: readonly string[]) => void;
  end: () => void;
};

const SelectionContext = React.createContext<Selection | null>(null);

/** `null` outside a reading-lists board (the rows then never select). */
export function useListSelection(): Selection | null {
  return React.useContext(SelectionContext);
}

export function ListSelectionProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [selecting, setSelecting] = React.useState(false);
  const [selected, setSelected] = React.useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const value = React.useMemo<Selection>(
    () => ({
      selecting,
      selected,
      begin: (ids) => {
        setSelecting(true);
        if (ids) setSelected((s) => new Set([...s, ...ids]));
      },
      toggle: (id) =>
        setSelected((s) => {
          const next = new Set(s);
          if (!next.delete(id)) next.add(id);
          return next;
        }),
      setAll: (ids) => setSelected(new Set(ids)),
      end: () => {
        setSelecting(false);
        setSelected(new Set());
      },
    }),
    [selecting, selected],
  );
  return (
    <SelectionContext.Provider value={value}>
      {children}
    </SelectionContext.Provider>
  );
}

/** The toolbar's Select button; hidden while selecting (the bar has Done). */
export function SelectListsButton({ disabled }: { disabled?: boolean }) {
  const sel = useListSelection();
  if (!sel || sel.selecting) return null;
  return (
    <Button
      type="button"
      variant="outline"
      className="h-9"
      disabled={disabled}
      onClick={() => sel.begin()}
    >
      <ListChecks className="mr-1.5 size-4" /> Select
    </Button>
  );
}

/** The bar under the toolbar while selecting: the count, Select all,
 *  Delete, Done. `views` is every list on the board. */
export function ListSelectionBar({ views }: { views: SavedViewView[] }) {
  const sel = useListSelection();
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const deletable = React.useMemo(() => views.filter(canDeleteList), [views]);
  // Only what is still on the board counts: a list deleted elsewhere
  // drops out of the selection on its own.
  const chosen = React.useMemo(
    () => deletable.filter((v) => sel?.selected.has(v.id)),
    [deletable, sel?.selected],
  );
  if (!sel?.selecting) return null;
  const allChosen = deletable.length > 0 && chosen.length === deletable.length;
  return (
    <>
      <div
        role="region"
        aria-label="Selected reading lists"
        data-list-selection-bar=""
        className="border-border bg-card sticky top-[calc(var(--topbar-h)+0.5rem)] z-20 flex flex-wrap items-center gap-2 rounded-lg border px-3 py-2"
      >
        <p className="text-sm font-medium tabular-nums" aria-live="polite">
          {chosen.length} selected
        </p>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={deletable.length === 0}
          onClick={() =>
            sel.setAll(allChosen ? [] : deletable.map((v) => v.id))
          }
        >
          {allChosen ? "Select none" : `Select all ${deletable.length}`}
        </Button>
        <div className="ml-auto flex items-center gap-2">
          <Button
            type="button"
            variant="destructive"
            size="sm"
            disabled={chosen.length === 0}
            onClick={() => setConfirmOpen(true)}
          >
            <Trash2 className="size-4" /> Delete…
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={sel.end}>
            <X className="size-4" /> Done
          </Button>
        </div>
      </div>
      <DeleteListsDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        views={chosen}
        onDeleted={(ids, allGone) => {
          if (allGone) sel.end();
          else sel.setAll(chosen.map((v) => v.id).filter((id) => !ids.has(id)));
        }}
      />
    </>
  );
}

const NAMES_SHOWN = 6;

/** The confirmation for deleting reading lists, one or many; it runs
 *  the deletes itself. Also the single Delete in a list's ⋯ menu. */
export function DeleteListsDialog({
  open,
  onOpenChange,
  views,
  onDeleted,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  views: SavedViewView[];
  /** The ids that went, and whether that was all of them. */
  onDeleted?: (ids: ReadonlySet<string>, allGone: boolean) => void;
}) {
  const qc = useQueryClient();
  const [progress, setProgress] = React.useState<number | null>(null);
  const running = progress !== null;

  async function run() {
    const targets = views;
    const gone = new Set<string>();
    setProgress(0);
    for (const [i, view] of targets.entries()) {
      setProgress(i);
      try {
        await apiMutate({
          path: `/me/saved-views/${view.id}`,
          method: "DELETE",
        });
        gone.add(view.id);
      } catch {
        // Counted below; the list stays selected for another try.
      }
    }
    setProgress(null);
    await Promise.all([
      qc.invalidateQueries({ queryKey: ["saved-views"] }),
      qc.invalidateQueries({ queryKey: queryKeys.cblLists }),
      qc.invalidateQueries({ queryKey: queryKeys.sidebarLayout }),
      qc.invalidateQueries({ queryKey: queryKeys.viewCovers }),
      qc.invalidateQueries({ queryKey: queryKeys.viewGroups("cbl") }),
    ]);
    const result = bulkDeleteResult(gone.size, targets.length - gone.size);
    if (result.ok) toast.success(result.message);
    else toast.error(result.message);
    onOpenChange(false);
    onDeleted?.(gone, gone.size === targets.length);
  }

  const extra = views.length - NAMES_SHOWN;
  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        // Not while it runs: the loop would carry on unseen.
        if (!running) onOpenChange(next);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{bulkDeleteTitle(views)}</AlertDialogTitle>
          <AlertDialogDescription>
            {views.length === 1 ? "The list is" : "The lists are"} removed with{" "}
            {views.length === 1 ? "its" : "their"} chapters, matches and links
            to other lists. This can&rsquo;t be undone. Your comics and what you
            have read stay as they are.
          </AlertDialogDescription>
        </AlertDialogHeader>
        {views.length > 1 ? (
          <ul className="text-muted-foreground max-h-40 list-disc space-y-0.5 overflow-y-auto pl-5 text-sm">
            {views.slice(0, NAMES_SHOWN).map((v) => (
              <li key={v.id} className="break-words">
                {v.name}
              </li>
            ))}
            {extra > 0 ? <li>and {extra} more</li> : null}
          </ul>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={running}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={running || views.length === 0}
            onClick={(e) => {
              // Keep the dialog open while the deletes run.
              e.preventDefault();
              void run();
            }}
          >
            {running
              ? `Deleting ${progress + 1} of ${views.length}…`
              : views.length === 1
                ? "Delete list"
                : `Delete ${views.length} lists`}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
