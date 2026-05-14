"use client";

import * as React from "react";
import Link from "next/link";
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
import { GripVertical, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { IssueCard } from "@/components/library/IssueCard";
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
import { useCollectionEntries } from "@/lib/api/queries";
import {
  useDeleteCollection,
  useRemoveCollectionEntry,
  useReorderCollectionEntries,
  useUpdateCollection,
} from "@/lib/api/mutations";
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
  const entriesQ = useCollectionEntries(savedView.id, { limit: 200 });
  const reorder = useReorderCollectionEntries(savedView.id);
  const remove = useRemoveCollectionEntry(savedView.id);
  const update = useUpdateCollection(savedView.id);
  const del = useDeleteCollection(savedView.id);
  const [editOpen, setEditOpen] = React.useState(false);
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

  const allEntries = entriesQ.data?.items ?? [];
  const total = entriesQ.data?.total ?? allEntries.length;
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
          </>
        }
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
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          <SortableContext items={renderIds} strategy={rectSortingStrategy}>
            <ul role="list" className="grid gap-3" style={gridStyle}>
              {orderedEntries.map((entry) => (
                <SortableEntry
                  key={entry.id}
                  entry={entry}
                  onRemove={() => remove.mutate({ entryId: entry.id })}
                />
              ))}
            </ul>
          </SortableContext>
        </DndContext>
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
}: {
  entry: CollectionEntryView;
  onRemove: () => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: entry.id });
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
      <DragHandle attributes={attributes} listeners={listeners} />
      {entry.series ? (
        <SeriesCard
          series={entry.series}
          size="md"
          extraActions={[removeAction]}
        />
      ) : entry.issue ? (
        <IssueCard issue={entry.issue} extraActions={[removeAction]} />
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
      className="bg-background/85 absolute top-2 right-2 z-20 inline-flex h-8 w-8 cursor-grab touch-none items-center justify-center rounded-full opacity-0 ring-1 shadow-sm ring-black/10 backdrop-blur transition group-hover:opacity-100 focus-visible:opacity-100 active:cursor-grabbing dark:ring-white/10"
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
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setName(savedView.name);
      // eslint-disable-next-line react-hooks/set-state-in-effect
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
