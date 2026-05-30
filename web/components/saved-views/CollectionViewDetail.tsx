"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  rectSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  Check,
  Circle,
  FileCog,
  FolderPlus,
  GripVertical,
  ListChecks,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import { BulkArchiveEditDialog } from "@/components/library/BulkArchiveEditDialog";
import { IssueCard } from "@/components/library/IssueCard";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { SeriesCard } from "@/components/library/SeriesCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { useCardSize } from "@/components/library/use-card-size";
import { useCollectionEntriesInfinite, useMe } from "@/lib/api/queries";
import {
  useBulkMarkProgress,
  useBulkRemoveFromCollection,
  useDeleteCollection,
  useRemoveCollectionEntry,
  useReorderCollectionEntries,
  useUpdateCollection,
} from "@/lib/api/mutations";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useSelection } from "@/lib/selection/use-selection";
import { TOAST } from "@/lib/api/toast-strings";
import { cn } from "@/lib/utils";
import type { CollectionEntryView, SavedViewView } from "@/lib/api/types";

import { ViewHeader } from "./ViewHeader";

const CARD_SIZE_MIN = 120;
const CARD_SIZE_MAX = 280;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 160;
const CARD_SIZE_STORAGE_KEY = "folio.savedView.cardSize";
const WANT_TO_READ_KEY = "want_to_read";

/** Detail page for a user collection. Mirrors `CblViewDetail` minus the
 *  refresh/export/import machinery — collections are user-curated lists
 *  of mixed series + issue refs with drag-reorder. Want to Read (the
 *  per-user system collection) renders here too; its Edit dialog hides
 *  the Delete button because the server rejects WTR deletion with 409. */
export function CollectionViewDetail({
  savedView,
}: {
  savedView: SavedViewView;
}) {
  const router = useRouter();
  const isWantToRead = savedView.system_key === WANT_TO_READ_KEY;
  // Auto-walk every page. Reorder semantics require the full list of
  // entry ids — sending a half-loaded view to `useReorderCollectionEntries`
  // would wipe everything past the load window. The fetch runs in 200-entry
  // pages; for typical user-curated collections this is one round-trip.
  const entriesQ = useCollectionEntriesInfinite(savedView.id);
  React.useEffect(() => {
    if (entriesQ.hasNextPage && !entriesQ.isFetchingNextPage) {
      void entriesQ.fetchNextPage();
    }
  }, [entriesQ]);
  const reorder = useReorderCollectionEntries(savedView.id);
  const remove = useRemoveCollectionEntry(savedView.id);
  const update = useUpdateCollection(savedView.id);
  const del = useDeleteCollection(savedView.id);
  // Multi-select bulk actions on the collection detail page.
  // Selection tracks `collection_entries.id` (the row PK) so we can
  // resolve each selected entry to its `(entry_kind, ref_id)` pair
  // when firing the bulk endpoints. Mixed-kind selections fire two
  // calls (one for series targets, one for issue targets) for the
  // mark-read/unread action; the bulk-add and bulk-remove endpoints
  // accept the mixed shape natively.
  const bulkMark = useBulkMarkProgress();
  const bulkRemove = useBulkRemoveFromCollection(savedView.id);
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const [confirmRemove, setConfirmRemove] = React.useState(false);
  const [archiveEditOpen, setArchiveEditOpen] = React.useState(false);
  const [editOpen, setEditOpen] = React.useState(false);
  const isAdmin = useMe().data?.role === "admin";
  const [confirmDelete, setConfirmDelete] = React.useState(false);

  const [optimisticOrder, setOptimisticOrder] = React.useState<string[] | null>(
    null,
  );
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const allEntries = React.useMemo<CollectionEntryView[]>(
    () => entriesQ.data?.pages.flatMap((p) => p.items) ?? [],
    [entriesQ.data],
  );
  const total = entriesQ.data?.pages[0]?.total ?? allEntries.length;
  // Sort by the optimistic order when the user is mid-drag; otherwise
  // server order. Optimistic order references the entry ids that exist
  // server-side so the cache invalidation flushes it cleanly.
  const entryById = React.useMemo(() => {
    const m = new Map<string, CollectionEntryView>();
    for (const e of allEntries) m.set(e.id, e);
    return m;
  }, [allEntries]);
  const serverOrderIds = React.useMemo(
    () => allEntries.map((e) => e.id),
    [allEntries],
  );
  const renderIds = optimisticOrder ?? serverOrderIds;
  const orderedEntries = renderIds
    .map((id) => entryById.get(id))
    .filter((e): e is CollectionEntryView => e !== undefined);

  const selection = useSelection(orderedEntries);
  // Helper: resolve a selected entry.id → its (entry_kind, ref_id).
  // Used by all three bulk actions on this surface.
  const selectedTargets = React.useMemo(() => {
    const out: { entry_kind: "issue" | "series"; ref_id: string }[] = [];
    for (const id of selection.selected) {
      const entry = entryById.get(id);
      if (!entry) continue;
      if (entry.entry_kind === "issue" && entry.issue) {
        out.push({ entry_kind: "issue", ref_id: entry.issue.id });
      } else if (entry.entry_kind === "series" && entry.series) {
        out.push({ entry_kind: "series", ref_id: entry.series.id });
      }
    }
    return out;
  }, [selection.selected, entryById]);
  // Issue-only target subset — bulk-mark-progress only acts on issues.
  // Series cards in the selection are silently skipped (a toast tells
  // the user how many fired vs. skipped).
  const selectedIssueIds = React.useMemo(
    () =>
      selectedTargets
        .filter((t) => t.entry_kind === "issue")
        .map((t) => t.ref_id),
    [selectedTargets],
  );

  // Esc exits select mode; Cmd/Ctrl+A selects every loaded entry.
  // Both gated on `selectMode` so other handlers are free when the
  // toolbar isn't up. `shouldSkipHotkey` keeps the bindings dormant
  // while focus is in a form field.
  const selectButtonRef = React.useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = React.useRef(false);
  React.useEffect(() => {
    if (!selection.selectMode) return;
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        selection.exit();
      } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        selection.selectAll();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selection]);
  // Restore focus to the Select trigger after leaving select mode.
  React.useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);

  const runBulkMark = (finished: boolean) => {
    if (selectedIssueIds.length === 0) {
      toast.info("Mark read / unread applies to issues; series cards skipped");
      return;
    }
    bulkMark.mutate(
      { issue_ids: selectedIssueIds, finished },
      { onSuccess: () => selection.clear() },
    );
  };
  const runBulkRemove = () => {
    if (selectedTargets.length === 0) return;
    bulkRemove.mutate(
      { members: selectedTargets },
      {
        onSuccess: () => {
          selection.exit();
          setConfirmRemove(false);
        },
      },
    );
  };

  // Gate: DnD is off while pagination is unresolved (a reorder
  // would clip the unloaded tail) and while in select mode (so the
  // drag-vs-toggle gesture stays unambiguous). Threaded into each
  // `<SortableEntry>`'s `useSortable({ disabled })` so sensors
  // stay armed but the items themselves don't respond.
  const dndDisabled = entriesQ.hasNextPage || selection.selectMode;

  function handleDragEnd(ev: DragEndEvent) {
    const { active, over } = ev;
    if (!over || active.id === over.id) return;
    const oldIndex = renderIds.indexOf(String(active.id));
    const newIndex = renderIds.indexOf(String(over.id));
    if (oldIndex < 0 || newIndex < 0) return;
    const next = arrayMove(renderIds, oldIndex, newIndex);
    setOptimisticOrder(next);
    reorder.mutate(
      { entry_ids: next },
      {
        onError: () => {
          setOptimisticOrder(null);
          toast.error("Couldn't save the new order");
        },
        onSuccess: () => {
          // Clear local override once the server-fresh order lands —
          // otherwise a stale optimistic copy lingers if the user
          // edits and we re-fetch.
          setOptimisticOrder(null);
        },
      },
    );
  }

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
            <Badge variant="secondary">
              {total} item{total === 1 ? "" : "s"}
            </Badge>
            <CardSizeOptions
              cardSize={cardSize}
              onCardSize={setCardSize}
              min={CARD_SIZE_MIN}
              max={CARD_SIZE_MAX}
              step={CARD_SIZE_STEP}
              defaultSize={CARD_SIZE_DEFAULT}
            />
            {orderedEntries.length > 0 && (
              <Button
                ref={selectButtonRef}
                variant="outline"
                size="sm"
                onClick={() => selection.enter()}
                aria-label="Enter select mode"
                aria-hidden={selection.selectMode}
                tabIndex={selection.selectMode ? -1 : 0}
                disabled={selection.selectMode}
                className={cn(
                  "transition-opacity duration-150",
                  selection.selectMode &&
                    "pointer-events-none invisible opacity-0",
                )}
              >
                <ListChecks className="mr-1.5 h-4 w-4" />
                Select
              </Button>
            )}
          </>
        }
      />

      <SelectionToolbar
        open={selection.selectMode}
        count={selection.count}
        total={orderedEntries.length}
        primary={[
          {
            id: "mark-read",
            label: "Mark read",
            icon: Check,
            onClick: () => runBulkMark(true),
          },
          {
            id: "mark-unread",
            label: "Mark unread",
            icon: Circle,
            onClick: () => runBulkMark(false),
          },
        ]}
        overflow={[
          {
            id: "add-to-collection",
            label: "Add to collection…",
            icon: FolderPlus,
            onClick: () => setPickerOpen(true),
          },
          ...(isAdmin
            ? [
                {
                  id: "edit-archives",
                  label: "Edit archives…",
                  icon: FileCog,
                  onClick: () => setArchiveEditOpen(true),
                },
              ]
            : []),
          {
            id: "remove",
            label: "Remove from this collection",
            icon: Trash2,
            onClick: () => setConfirmRemove(true),
            destructive: true,
          },
        ]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={bulkMark.isPending || bulkRemove.isPending}
      />

      {entriesQ.isLoading ? (
        <div className="text-muted-foreground py-12 text-sm">Loading…</div>
      ) : entriesQ.isError ? (
        <div className="text-destructive rounded-md border p-4 text-sm">
          Failed to load collection entries.
        </div>
      ) : orderedEntries.length === 0 ? (
        <EmptyState isWantToRead={isWantToRead} />
      ) : (
        <>
          {/* Reorder needs the *full* id list — `useReorderCollectionEntries`
              wipes anything not in `entry_ids`. Disable DnD until
              pagination drains so a mid-load drag can't truncate the
              tail, and while in select mode so the drag-vs-toggle
              gesture stays unambiguous. The `sensors` prop must keep
              a stable size between renders (React useEffect-deps
              invariant inside dnd-kit's `useSensorSetup`), so we
              gate via per-item `disabled` + a guarded `onDragEnd`
              rather than swapping the sensors array. */}
          <DndContext
            sensors={sensors}
            collisionDetection={closestCenter}
            onDragEnd={dndDisabled ? undefined : handleDragEnd}
          >
            <SortableContext items={renderIds} strategy={rectSortingStrategy}>
              <ul role="list" className="grid gap-3" style={gridStyle}>
                {orderedEntries.map((entry) => (
                  <SortableEntry
                    key={entry.id}
                    entry={entry}
                    onRemove={() => remove.mutate({ entryId: entry.id })}
                    dndDisabled={dndDisabled}
                    selectMode={
                      selection.selectMode
                        ? {
                            isActive: true,
                            isSelected: selection.isSelected(entry.id),
                            onToggle: (ev) => selection.toggle(entry.id, ev),
                          }
                        : undefined
                    }
                    onEnterSelectMode={() => selection.toggle(entry.id)}
                  />
                ))}
              </ul>
            </SortableContext>
          </DndContext>
          {entriesQ.isFetchingNextPage ? (
            <p className="text-muted-foreground mt-3 text-center text-xs">
              Loading more ({orderedEntries.length} of {total})…
            </p>
          ) : null}
        </>
      )}

      <EditCollectionDialog
        open={editOpen}
        onOpenChange={setEditOpen}
        savedView={savedView}
        canDelete={!isWantToRead}
        onSave={(body) =>
          update.mutate(body, { onSuccess: () => setEditOpen(false) })
        }
        onDeleteRequest={() => {
          setEditOpen(false);
          setConfirmDelete(true);
        }}
        saving={update.isPending}
      />

      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this collection?</AlertDialogTitle>
            <AlertDialogDescription>
              Entries are removed from the collection but the underlying series
              and issues are not touched. This can&apos;t be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() =>
                del.mutate(undefined, {
                  onSuccess: () => {
                    setConfirmDelete(false);
                    router.push("/collections");
                  },
                })
              }
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* Bulk-remove confirmation — destructive multi-select action.
       *  The cards stay in the library; only the collection-membership
       *  rows are removed. */}
      <AlertDialog open={confirmRemove} onOpenChange={setConfirmRemove}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Remove {selectedTargets.length} item
              {selectedTargets.length === 1 ? "" : "s"} from this collection?
            </AlertDialogTitle>
            <AlertDialogDescription>
              The items stay in your library; only their membership in &ldquo;
              {savedView.name}&rdquo; is removed.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={runBulkRemove}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <BulkAddToCollectionDialog
        open={pickerOpen}
        onOpenChange={(next) => {
          setPickerOpen(next);
          if (!next) selection.clear();
        }}
        targets={selectedTargets}
      />
      <BulkArchiveEditDialog
        open={archiveEditOpen}
        onOpenChange={(next) => {
          setArchiveEditOpen(next);
          if (!next) selection.clear();
        }}
        issueIds={selectedIssueIds}
      />
    </div>
  );
}

function EmptyState({ isWantToRead }: { isWantToRead: boolean }) {
  return (
    <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
      {isWantToRead
        ? "Nothing in Want to Read yet. Use the kebab menu on any series or issue cover to add it here."
        : "This collection is empty. Use the kebab menu on a series or issue cover to add items."}
    </div>
  );
}

function SortableEntry({
  entry,
  onRemove,
  dndDisabled,
  selectMode,
  onEnterSelectMode,
}: {
  entry: CollectionEntryView;
  onRemove: () => void;
  /** When true, the item is mounted in the sortable context but
   *  doesn't respond to drag — preserves a stable sensors-array
   *  size across renders (the dnd-kit warning we were tripping). */
  dndDisabled?: boolean;
  selectMode?: {
    isActive: boolean;
    isSelected: boolean;
    onToggle: (ev?: React.MouseEvent) => void;
  };
  /** M6 long-press-sheet "Select" entry callback. Forwarded to
   *  whichever card type renders for this entry. */
  onEnterSelectMode?: (entryId: string) => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: entry.id, disabled: dndDisabled });
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 10 : undefined,
  };
  // "Remove from this collection" is appended to the shared cover-menu
  // (mark read/unread + add-to-collection) via the new `extraActions`
  // prop on SeriesCard / IssueCard. Reusing those components means the
  // play overlay + standard affordances stay consistent across every
  // surface that paints covers.
  const removeAction = {
    label: "Remove from this collection",
    onSelect: onRemove,
    destructive: true,
  };
  return (
    <li
      ref={setNodeRef}
      style={style}
      className={cn("group relative", isDragging && "opacity-70")}
    >
      {/* Drag handle hidden while in select mode — the handle's
       *  drag gesture would conflict with tap-to-toggle. */}
      {!selectMode?.isActive && (
        <DragHandle attributes={attributes} listeners={listeners} />
      )}
      {entry.series ? (
        <SeriesCard
          series={entry.series}
          size="md"
          extraActions={[removeAction]}
          selectMode={selectMode}
          onEnterSelectMode={
            onEnterSelectMode ? () => onEnterSelectMode(entry.id) : undefined
          }
        />
      ) : entry.issue ? (
        <IssueCard
          issue={entry.issue}
          extraActions={[removeAction]}
          selectMode={selectMode}
          onEnterSelectMode={
            onEnterSelectMode ? () => onEnterSelectMode(entry.id) : undefined
          }
        />
      ) : (
        <MissingEntry onRemove={onRemove} />
      )}
    </li>
  );
}

function DragHandle({
  attributes,
  listeners,
}: {
  attributes: React.HTMLAttributes<HTMLButtonElement>;
  listeners: React.DOMAttributes<HTMLButtonElement> | undefined;
}) {
  return (
    <button
      type="button"
      aria-label="Drag to reorder"
      title="Drag to reorder"
      // The handle sits at top-right of the card; the kebab is top-left.
      // `touch-none` prevents the page from scrolling while dragging on
      // touch devices.
      className="bg-background/85 absolute top-2 right-2 z-20 inline-flex h-8 w-8 cursor-grab touch-none items-center justify-center rounded-full opacity-0 shadow-sm ring-1 ring-black/10 backdrop-blur transition group-hover:opacity-100 focus-visible:opacity-100 active:cursor-grabbing dark:ring-white/10"
      {...attributes}
      {...listeners}
    >
      <GripVertical className="h-4 w-4" aria-hidden="true" />
    </button>
  );
}

function MissingEntry({ onRemove }: { onRemove: () => void }) {
  return (
    <div className="border-border/60 bg-muted/20 flex aspect-[2/3] w-full flex-col items-center justify-center gap-2 rounded-md border border-dashed p-3 text-center">
      <span className="text-muted-foreground text-xs">
        Underlying item is missing.
      </span>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={onRemove}
        className="gap-1"
      >
        <Trash2 className="h-3.5 w-3.5" /> Remove
      </Button>
    </div>
  );
}

function EditCollectionDialog({
  open,
  onOpenChange,
  savedView,
  canDelete,
  onSave,
  onDeleteRequest,
  saving,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  savedView: SavedViewView;
  canDelete: boolean;
  onSave: (body: { name?: string; description?: string }) => void;
  onDeleteRequest: () => void;
  saving: boolean;
}) {
  const [name, setName] = React.useState(savedView.name);
  const [description, setDescription] = React.useState(
    savedView.description ?? "",
  );
  // Re-seed on each open so a "Cancel" out of mid-edit doesn't poison
  // the next session with stale local state.
  React.useEffect(() => {
    if (open) {
      // Seed form fields from the saved view when the dialog reopens.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setName(savedView.name);
      setDescription(savedView.description ?? "");
    }
  }, [open, savedView.name, savedView.description]);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      toast.error(TOAST.NAME_REQUIRED);
      return;
    }
    onSave({
      // Want to Read's name is fixed; the server ignores the field but
      // we skip sending it anyway so the request body matches user
      // intent.
      ...(savedView.system_key === WANT_TO_READ_KEY
        ? {}
        : { name: trimmedName }),
      description,
    });
  }

  const isWantToRead = savedView.system_key === WANT_TO_READ_KEY;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit collection</DialogTitle>
          <DialogDescription>
            {isWantToRead
              ? "Want to Read is your built-in saved-for-later list. You can edit the description, but the name is fixed."
              : "Rename or redescribe this collection."}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="collection-name">Name</Label>
            <Input
              id="collection-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={isWantToRead}
              required
              maxLength={200}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="collection-description">Description</Label>
            <Textarea
              id="collection-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={3}
              placeholder="Optional"
            />
          </div>
          <DialogFooter className="flex items-center justify-between gap-2 sm:justify-between">
            {canDelete ? (
              <Button
                type="button"
                variant="ghost"
                className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                onClick={onDeleteRequest}
              >
                <Trash2 className="mr-1 h-4 w-4" /> Delete
              </Button>
            ) : (
              <span />
            )}
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="ghost"
                onClick={() => onOpenChange(false)}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={saving}>
                Save
              </Button>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
