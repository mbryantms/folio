"use client";

/**
 * Editing a page's rails on the page itself (inline rail editing).
 *
 * - `usePageRailEdit` owns the order: move a rail, set a dragged order,
 *   remove a rail with Undo. Every change saves at once.
 * - `RailMenu` is the ⋯ menu in a rail's header: Move up / Move down /
 *   Open view / Remove from page.
 * - `RearrangeRails` replaces the rails with their title bars so a whole
 *   page of them fits on screen: drag by the grip (touch included), or
 *   use the arrows.
 *
 * Settings → Pages keeps the same controls for anyone who looks there.
 */

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
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import {
  ArrowDown,
  ArrowUp,
  ExternalLink,
  GripVertical,
  MoreHorizontal,
  Plus,
  X,
} from "lucide-react";
import Link from "next/link";
import * as React from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Button } from "@/components/ui/button";
import { useReorderSavedViews, useTogglePinOnPage } from "@/lib/api/mutations";
import type { SavedViewView } from "@/lib/api/types";
import { moveRail, restoreRail } from "@/lib/pages/rail-order";
import { useHeldOrder } from "@/lib/ui/held-order";
import {
  DRAG_LIFT_CLASS,
  sortableStyle,
  useSpringSort,
} from "@/lib/ui/springy-drag";
import { showUndoToast } from "@/lib/ui/undo-toast";
import { cn } from "@/lib/utils";

import { ManagePinsDialog } from "./ManagePinsDialog";
import { viewDetailHref } from "./SavedViewRail";

export type PageRailEdit = {
  /** The rails in the order to draw them (a dropped order holds until
   *  the server agrees). */
  ordered: SavedViewView[];
  move: (id: string, delta: number) => void;
  setOrder: (ids: string[]) => void;
  remove: (view: SavedViewView) => void;
  /** The page shows its rails as title bars to reorder. */
  rearranging: boolean;
  setRearranging: (on: boolean) => void;
};

const NO_RAILS: SavedViewView[] = [];

export function usePageRailEdit(
  pageId: string,
  pinned: SavedViewView[] | undefined,
): PageRailEdit {
  const items = pinned ?? NO_RAILS;
  const [rearranging, setRearranging] = React.useState(false);
  const reorder = useReorderSavedViews();
  const toggle = useTogglePinOnPage();
  const serverIds = React.useMemo(() => items.map((v) => v.id), [items]);
  const held = useHeldOrder(serverIds);
  const ids = held.order;
  const ordered = React.useMemo(() => {
    const byId = new Map(items.map((v) => [v.id, v]));
    return ids
      .map((id) => byId.get(id))
      .filter((v): v is SavedViewView => v !== undefined);
  }, [ids, items]);

  const setOrder = (next: string[]) => {
    held.hold(next);
    reorder.mutate(
      { page_id: pageId, view_ids: next },
      { onError: held.release, onSuccess: () => held.releaseLater(next) },
    );
  };

  const move = (id: string, delta: number) => {
    const next = moveRail(ids, id, delta);
    if (next !== ids) setOrder([...next]);
  };

  const remove = (view: SavedViewView) => {
    const before = [...ids];
    const index = before.indexOf(view.id);
    toggle.mutate(
      { viewId: view.id, pageId, pinned: false },
      {
        onSuccess: () =>
          showUndoToast({
            message: `Removed "${view.name}" from this page`,
            restoredMessage: `Put "${view.name}" back`,
            failedMessage: "Couldn't put the rail back",
            onUndo: async () => {
              await toggle.mutateAsync({
                viewId: view.id,
                pageId,
                pinned: true,
              });
              // A fresh pin lands last; put it where it was.
              await reorder.mutateAsync({
                page_id: pageId,
                view_ids: restoreRail(
                  before.filter((id) => id !== view.id),
                  view.id,
                  index,
                ),
              });
            },
          }),
      },
    );
  };

  return { ordered, move, setOrder, remove, rearranging, setRearranging };
}

/** The ⋯ menu in a rail's header. */
export function RailMenu({
  view,
  index,
  count,
  edit,
}: {
  view: SavedViewView;
  index: number;
  count: number;
  edit: PageRailEdit;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="text-muted-foreground size-8 shrink-0"
          aria-label={`${view.name} rail actions`}
        >
          <MoreHorizontal className="size-4" aria-hidden />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem
          disabled={index === 0}
          onSelect={() => edit.move(view.id, -1)}
        >
          <ArrowUp className="size-4" aria-hidden />
          Move up
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={index >= count - 1}
          onSelect={() => edit.move(view.id, 1)}
        >
          <ArrowDown className="size-4" aria-hidden />
          Move down
        </DropdownMenuItem>
        <DropdownMenuItem asChild>
          <Link href={viewDetailHref(view)}>
            <ExternalLink className="size-4" aria-hidden />
            Open view
          </Link>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          variant="destructive"
          onSelect={() => edit.remove(view)}
        >
          <X className="size-4" aria-hidden />
          Remove from page
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The rails as title bars, for moving several at once. */
export function RearrangeRails({
  pageId,
  edit,
  onDone,
}: {
  pageId: string;
  edit: PageRailEdit;
  onDone: () => void;
}) {
  const [addOpen, setAddOpen] = React.useState(false);
  const ids = edit.ordered.map((v) => v.id);
  const sensors = useSensors(
    // The grip carries `touch-none`, so a finger on it drags instead of
    // scrolling the page; everywhere else on the row still scrolls.
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );
  const onDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    edit.setOrder(arrayMove(ids, from, to));
  };

  return (
    <section aria-label="Rearrange rails" data-rearrange-rails>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <p className="text-muted-foreground min-w-0 flex-1 basis-48 text-sm">
          Drag a rail by its grip, or use the arrows. Changes save as you go.
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setAddOpen(true)}
        >
          <Plus aria-hidden />
          Add rail
        </Button>
        <Button type="button" size="sm" onClick={onDone}>
          Done
        </Button>
      </div>
      {edit.ordered.length === 0 ? (
        <p className="text-muted-foreground border-border/60 rounded-lg border border-dashed p-6 text-sm">
          No rails on this page yet. Add one to get started.
        </p>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={onDragEnd}
        >
          <SortableContext items={ids} strategy={verticalListSortingStrategy}>
            <ol className="flex flex-col gap-2">
              {edit.ordered.map((view, index) => (
                <RearrangeRow
                  key={view.id}
                  view={view}
                  index={index}
                  count={edit.ordered.length}
                  edit={edit}
                />
              ))}
            </ol>
          </SortableContext>
        </DndContext>
      )}
      <ManagePinsDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        pageId={pageId}
      />
    </section>
  );
}

function RearrangeRow({
  view,
  index,
  count,
  edit,
}: {
  view: SavedViewView;
  index: number;
  count: number;
  edit: PageRailEdit;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: view.id, transition: useSpringSort() });
  return (
    <li
      ref={setNodeRef}
      style={sortableStyle({ transform, transition, isDragging })}
      className={cn(
        "border-border bg-card flex items-center gap-2 rounded-lg border py-2 pr-2 pl-1",
        isDragging && DRAG_LIFT_CLASS,
      )}
    >
      <button
        type="button"
        aria-label={`Drag ${view.name}`}
        className="text-muted-foreground hover:text-foreground hover:bg-secondary/50 flex size-9 shrink-0 cursor-grab touch-none items-center justify-center rounded-md transition-colors active:cursor-grabbing"
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(attributes as any)}
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(listeners as any)}
      >
        <GripVertical className="size-4" aria-hidden />
      </button>
      <span className="text-muted-foreground/70 w-5 shrink-0 text-right text-xs tabular-nums">
        {index + 1}
      </span>
      <span
        className="min-w-0 flex-1 truncate text-sm font-medium"
        title={view.name}
      >
        {view.name}
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-8 shrink-0"
        disabled={index === 0}
        aria-label={`Move ${view.name} up`}
        onClick={() => edit.move(view.id, -1)}
      >
        <ArrowUp className="size-4" aria-hidden />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-8 shrink-0"
        disabled={index >= count - 1}
        aria-label={`Move ${view.name} down`}
        onClick={() => edit.move(view.id, 1)}
      >
        <ArrowDown className="size-4" aria-hidden />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="text-muted-foreground hover:text-destructive size-8 shrink-0"
        aria-label={`Remove ${view.name} from page`}
        onClick={() => edit.remove(view)}
      >
        <X className="size-4" aria-hidden />
      </Button>
    </li>
  );
}
