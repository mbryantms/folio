"use client";

import * as React from "react";
import dynamic from "next/dynamic";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
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
import {
  Check,
  Circle,
  Download,
  FileCog,
  FolderPlus,
  GripVertical,
  HardDriveDownload,
  ListChecks,
  Play,
  Trash2,
} from "lucide-react";

import { DropdownMenuItem } from "@/components/folio/dropdown-menu";
import { toast } from "sonner";

import { BulkAddToCollectionDialog } from "@/components/collections/BulkAddToCollectionDialog";
import { IssueCard } from "@/components/library/IssueCard";
import { CoverGridSkeleton } from "@/components/skeletons/LoadingShapes";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { SeriesCard } from "@/components/library/SeriesCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  CoverSizeFields,
  CoverSizeOptions,
} from "@/components/library/CoverSizeOptions";
import {
  PhoneControlsBar,
  PhoneMenuAction,
} from "@/components/library/PhoneControlsBar";
import {
  jsonFetch,
  useCollectionEntriesInfinite,
  useCollectionNextUp,
  useMe,
} from "@/lib/api/queries";
import { LoadMoreError } from "@/components/folio/infinite-sentinel";
import { RailError } from "@/components/saved-views/RailError";
import {
  useBulkMarkProgress,
  useBulkRemoveFromCollection,
  useDeleteCollection,
  useRemoveCollectionEntry,
  useReorderCollectionEntries,
  useUpdateCollection,
} from "@/lib/api/mutations";
import { skippedEntryIds } from "@/lib/library/bulk-archive-skips";
import { invalidateCollectionEntries } from "@/lib/api/mutations";
import { apiMutate } from "@/lib/api/mutations/_core";
import {
  entryToMember,
  fetchCollectionSnapshot,
  snapshotFromEntries,
  type CollectionMember,
} from "@/lib/collections/recreate";
import { useCollectionDeleteUndo } from "@/lib/collections/use-collection-undo";
import { COVER_GRID_STYLE } from "@/lib/library/cover-size";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useSelection } from "@/lib/selection/use-selection";
import { TOAST } from "@/lib/api/toast-strings";
import { readerUrl } from "@/lib/urls";
import { issueLabel } from "@/lib/format";
import { restoreOrder, showUndoToast } from "@/lib/ui/undo-toast";
import { cn } from "@/lib/utils";
import type {
  CollectionEntriesView,
  CollectionEntryView,
  SavedViewView,
} from "@/lib/api/types";
import {
  DRAG_LIFT_CLASS,
  grabAnywhere,
  sortableStyle,
  useSpringSort,
} from "@/lib/ui/springy-drag";

import { ViewHeader } from "./ViewHeader";

// Heavy archive-edit dialog (+dnd) — lazy so it stays out of the collection
// view's initial bundle; the chunk loads on first open (G6).
// "Download for offline" dialog (WP-4.6) — lazy, like the archive dialog.
const OfflineDownloadDialog = dynamic(
  () =>
    import("@/components/offline/OfflineDownloadDialog").then(
      (m) => m.OfflineDownloadDialog,
    ),
  { ssr: false },
);

const BulkArchiveEditDialog = dynamic(
  () =>
    import("@/components/library/BulkArchiveEditDialog").then(
      (m) => m.BulkArchiveEditDialog,
    ),
  { ssr: false },
);

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
  // A failed page parks the walk (no request loop) until its Retry.
  const entriesQ = useCollectionEntriesInfinite(savedView.id);
  React.useEffect(() => {
    if (
      entriesQ.hasNextPage &&
      !entriesQ.isFetchingNextPage &&
      !entriesQ.isFetchNextPageError
    ) {
      void entriesQ.fetchNextPage();
    }
  }, [entriesQ]);
  const qc = useQueryClient();
  const reorder = useReorderCollectionEntries(savedView.id);
  const remove = useRemoveCollectionEntry(savedView.id);
  const update = useUpdateCollection(savedView.id);
  // Silent delete + an Undo toast (audit B6): the detail page already
  // auto-walks every entry page, so the snapshot below is complete and
  // free — no extra fetch needed.
  const del = useDeleteCollection(savedView.id, { silent: true });
  const showDeleteUndo = useCollectionDeleteUndo();
  const nextUpQ = useCollectionNextUp(savedView.id);
  const [offlineOpen, setOfflineOpen] = React.useState(false);
  const [offlineMounted, setOfflineMounted] = React.useState(false);
  if (offlineOpen && !offlineMounted) setOfflineMounted(true);
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
  const [archiveEditOpen, setArchiveEditOpen] = React.useState(false);
  const [editOpen, setEditOpen] = React.useState(false);
  // Mount the lazy archive-edit dialog on first open; keep it mounted so its
  // close animation still runs (G6).
  const [archiveEditMounted, setArchiveEditMounted] = React.useState(false);
  if (archiveEditOpen && !archiveEditMounted) setArchiveEditMounted(true);
  const isAdmin = useMe().data?.role === "admin";

  const [optimisticOrder, setOptimisticOrder] = React.useState<string[] | null>(
    null,
  );

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
  // Hold the dropped order until the refetched server order matches it.
  // Clearing it on the mutation's success instead let the stale cached
  // order render for a frame or two before the refetch landed — the
  // dropped card flashed back to its old slot first.
  if (optimisticOrder && sameOrder(optimisticOrder, serverOrderIds)) {
    setOptimisticOrder(null);
  }
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
  const selectedSeriesCount = selectedTargets.length - selectedIssueIds.length;

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
      toast.info("Mark issues read/unread only applies to issue cards.");
      return;
    }
    bulkMark.mutate(
      { issue_ids: selectedIssueIds, finished },
      {
        onSuccess: () => {
          if (selectedSeriesCount > 0) {
            toast.info(
              `Updated ${selectedIssueIds.length} issue${selectedIssueIds.length === 1 ? "" : "s"}; skipped ${selectedSeriesCount} series ${selectedSeriesCount === 1 ? "card" : "cards"}.`,
            );
          }
          selection.clear();
        },
      },
    );
  };
  // Undo instead of confirm (folio-flair-1.0): the items leave at once;
  // Undo bulk-adds them back and replays the original order.
  const runBulkRemove = () => {
    if (selectedTargets.length === 0) return;
    const removed = selectedTargets;
    // Complete only once every page is loaded; otherwise restored items
    // land at the tail instead of their old slots.
    const originalOrder = allEntries
      .map(entryToMember)
      .filter((m): m is CollectionMember => m !== null);
    const collectionId = savedView.id;
    bulkRemove.mutate(
      { members: removed },
      {
        onSuccess: (data) => {
          selection.exit();
          const n = data?.removed ?? removed.length;
          if (n === 0) return;
          showUndoToast({
            // Replace the hook's own success toast (same id) rather than
            // stacking a second one.
            id: `bulk-remove-${collectionId}`,
            message: `Removed ${n} item${n === 1 ? "" : "s"} from "${savedView.name}"`,
            failedMessage: "Couldn't put the items back",
            onUndo: async () => {
              await restoreCollectionMembers(
                collectionId,
                removed,
                originalOrder,
              );
              invalidateCollectionEntries(qc, collectionId);
            },
          });
        },
      },
    );
  };

  // Delete fires straight from the Edit dialog; the Undo toast replays
  // a snapshot of name + description + ordered members taken first.
  const deleteCollection = async () => {
    let snapshot = snapshotFromEntries(
      savedView.name,
      savedView.description,
      allEntries,
    );
    if (entriesQ.hasNextPage) {
      // Unloaded tail: read the whole list so Undo restores all of it.
      try {
        snapshot = await fetchCollectionSnapshot(
          savedView.id,
          savedView.name,
          savedView.description,
        );
      } catch {
        // Fall back to what's loaded rather than blocking the delete.
      }
    }
    del.mutate(undefined, {
      onSuccess: () => {
        router.push("/collections");
        showDeleteUndo(snapshot);
      },
    });
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
          // Normally cleared above once the refetch matches; if the
          // server settles on a different order (a concurrent edit),
          // let it win after a moment rather than pinning ours.
          setTimeout(
            () => setOptimisticOrder((cur) => (cur === next ? null : cur)),
            4000,
          );
        },
      },
    );
  }

  return (
    <div className="space-y-6">
      {offlineMounted && (
        <OfflineDownloadDialog
          open={offlineOpen}
          onOpenChange={setOfflineOpen}
          target={{
            kind: "list",
            list: {
              id: savedView.id,
              kind: "collection",
              name: savedView.name,
              sourceId: savedView.id,
            },
            remaining:
              nextUpQ.data?.position != null
                ? nextUpQ.data.total - nextUpQ.data.position + 1
                : nextUpQ.data
                  ? 0
                  : null,
          }}
        />
      )}
      <ViewHeader
        view={savedView}
        onEdit={() => setEditOpen(true)}
        extraMenuItems={
          <>
            <DropdownMenuItem onSelect={() => setOfflineOpen(true)}>
              <HardDriveDownload className="mr-2 size-4" />
              Download for offline…
            </DropdownMenuItem>
            {/* Data-liberation (3.3): export the collection as a CBL
             *  reading list. Series entries expand to their issues
             *  server-side; a plain same-origin `<a download>` carries
             *  the session cookie. */}
            <DropdownMenuItem asChild>
              <a
                href={`/api/me/collections/${savedView.id}/export`}
                download
                title="Download as a .cbl reading list"
              >
                <Download className="mr-2 size-4" /> Export CBL
              </a>
            </DropdownMenuItem>
          </>
        }
        extraActions={
          <>
            <Badge variant="secondary">
              {total} item{total === 1 ? "" : "s"}
            </Badge>
            <CollectionContinueButton collectionId={savedView.id} />
            {(orderedEntries.length > 0 || selection.selectMode) && (
              <SelectModeButton
                ref={selectButtonRef}
                active={selection.selectMode}
                onEnter={() => selection.enter()}
                onExit={() => selection.exit()}
              />
            )}
            <CoverSizeOptions />
          </>
        }
        // Phone (< sm): the cluster above is hidden. Continue / Start
        // reading — the collection's primary action — stays one tap
        // away as the bar's icon button; the item count takes the
        // bar's leading slot; cover size + Select move into View options.
        phoneControlsPlacement="inline"
        phoneControls={
          <PhoneControlsBar
            sticky={false}
            hidden={selection.selectMode}
            leading={
              <p className="text-muted-foreground text-xs tabular-nums">
                {total} item{total === 1 ? "" : "s"}
              </p>
            }
            primary={
              <CollectionContinueButton collectionId={savedView.id} iconOnly />
            }
            menu={(close) => (
              <>
                <CoverSizeFields fieldId="phone-card-size" />
                {orderedEntries.length > 0 || selection.selectMode ? (
                  <PhoneMenuAction
                    icon={<ListChecks className="size-4" />}
                    onClick={() => {
                      close();
                      selection.enter();
                    }}
                  >
                    Select
                  </PhoneMenuAction>
                ) : null}
              </>
            )}
          />
        }
      />

      <SelectionToolbar
        open={selection.selectMode}
        count={selection.count}
        total={orderedEntries.length}
        primary={[
          {
            id: "mark-read",
            label: "Mark issues read",
            icon: Check,
            onClick: () => runBulkMark(true),
            disabled: selectedIssueIds.length === 0,
          },
          {
            id: "mark-unread",
            label: "Mark issues unread",
            icon: Circle,
            onClick: () => runBulkMark(false),
            disabled: selectedIssueIds.length === 0,
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
            onClick: runBulkRemove,
            destructive: true,
          },
        ]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={bulkMark.isPending || bulkRemove.isPending}
      />

      {entriesQ.isLoading ? (
        <CoverGridSkeleton className="gap-3" label="Loading collection…" />
      ) : entriesQ.isError && !entriesQ.data ? (
        <RailError
          message="Couldn't load this collection."
          onRetry={() => void entriesQ.refetch()}
        />
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
              <ul role="list" className="grid gap-3" style={COVER_GRID_STYLE}>
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
          <LoadMoreError query={entriesQ} />
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
          void deleteCollection();
        }}
        saving={update.isPending}
      />

      <BulkAddToCollectionDialog
        open={pickerOpen}
        onOpenChange={(next) => {
          setPickerOpen(next);
          if (!next) selection.clear();
        }}
        targets={selectedTargets}
      />
      {archiveEditMounted && (
        <BulkArchiveEditDialog
          open={archiveEditOpen}
          onOpenChange={(next) => {
            setArchiveEditOpen(next);
            if (!next) selection.clear();
          }}
          issueIds={selectedIssueIds}
          onResult={(res) => {
            setArchiveEditOpen(false);
            if (res.skipped.length > 0) {
              selection.replace(
                skippedEntryIds(
                  res.skipped,
                  orderedEntries.map((e) => ({
                    entryId: e.id,
                    issueId: e.entry_kind === "issue" ? e.issue?.id : undefined,
                  })),
                ),
              );
            } else {
              selection.clear();
            }
          }}
        />
      )}
    </div>
  );
}

/** "Start reading" / "Continue" — opens the reader on the first
 *  unfinished issue in the collection's reading order (series entries
 *  expanded server-side) with `?collection=` so next-up keeps following
 *  the collection (post-audit T4.1). Hidden while loading, when the
 *  collection is empty, and once everything is finished. */
const memberKey = (m: CollectionMember) => `${m.entry_kind}:${m.ref_id}`;

/** Undo for the bulk "Remove from this collection": bulk-add the removed
 *  members back (they land at the tail), then — when the pre-removal
 *  order is known — reorder the whole list so each one returns to its old
 *  slot. Entries added since keep their relative order after the
 *  originals. The reorder endpoint needs every entry id, so the current
 *  list is walked page by page first. Exported for tests. */
export async function restoreCollectionMembers(
  collectionId: string,
  removed: readonly CollectionMember[],
  originalOrder: readonly CollectionMember[],
): Promise<void> {
  await apiMutate({
    path: `/me/collections/${collectionId}/members/bulk-add`,
    method: "POST",
    body: { members: removed },
  });
  if (originalOrder.length === 0) return;

  const current: CollectionEntryView[] = [];
  let cursor: string | undefined;
  for (let page = 0; page < 30; page++) {
    const q = new URLSearchParams({ limit: "200" });
    if (cursor) q.set("cursor", cursor);
    const view = await jsonFetch<CollectionEntriesView>(
      `/me/collections/${collectionId}/entries?${q.toString()}`,
    );
    current.push(...view.items);
    if (!view.next_cursor) break;
    cursor = view.next_cursor;
  }
  const ordered = restoreOrder(current, originalOrder.map(memberKey), (e) => {
    const m = entryToMember(e);
    return m ? memberKey(m) : null;
  }).map((e) => e.id);
  if (ordered.every((id, i) => id === current[i]?.id)) return;
  await apiMutate({
    path: `/me/collections/${collectionId}/entries/reorder`,
    method: "POST",
    body: { entry_ids: ordered },
  });
}

export function CollectionContinueButton({
  collectionId,
  iconOnly = false,
}: {
  collectionId: string;
  /** Phone bar variant: a 36px square with just the Play glyph; the
   *  label survives in `aria-label` / `title`. */
  iconOnly?: boolean;
}) {
  const q = useCollectionNextUp(collectionId);
  const target = q.data?.target;
  if (!target) return null;
  const label = q.data?.started ? "Continue" : "Start reading";
  const ariaLabel = `${label}: ${issueLabel(target)}`;
  const href = readerUrl(target, { collection: collectionId });
  if (iconOnly) {
    return (
      <Button asChild size="icon" className="h-9 w-9 shrink-0">
        <Link href={href} aria-label={ariaLabel} title={label}>
          <Play className="size-4" aria-hidden="true" />
        </Link>
      </Button>
    );
  }
  return (
    // Toolbar-row convention: `size="toolbar"` beside CoverSizeOptions + Select.
    <Button asChild>
      <Link href={href} aria-label={ariaLabel}>
        <Play className="mr-1 size-4" aria-hidden="true" />
        {label}
      </Link>
    </Button>
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
  } = useSortable({
    id: entry.id,
    disabled: dndDisabled,
    transition: useSpringSort(),
  });
  const style = sortableStyle({ transform, transition, isDragging });
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
      // Pick the card up from anywhere (mouse / pen); the grip stays for
      // touch and keyboard.
      {...(selectMode?.isActive || dndDisabled ? {} : grabAnywhere(listeners))}
      className={cn("group relative rounded-md", isDragging && DRAG_LIFT_CLASS)}
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

function sameOrder(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((id, i) => id === b[i]);
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
      className="bg-background/85 absolute top-2 right-2 z-20 inline-flex size-8 cursor-grab touch-none items-center justify-center rounded-full opacity-0 shadow-sm ring-1 ring-black/10 backdrop-blur transition group-hover:opacity-100 focus-visible:opacity-100 active:cursor-grabbing dark:ring-white/10"
      {...attributes}
      {...listeners}
    >
      <GripVertical className="size-4" aria-hidden="true" />
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
        <Trash2 className="size-3.5" /> Remove
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
                <Trash2 className="mr-1 size-4" /> Delete
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
