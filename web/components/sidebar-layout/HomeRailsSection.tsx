"use client";

import * as React from "react";
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
import { CSS } from "@dnd-kit/utilities";
import {
  ChevronDown,
  ChevronUp,
  GripVertical,
  Plus,
  Sparkles,
  X,
} from "lucide-react";
import { toast } from "sonner";

import { railIconByKey } from "@/components/library/rail-icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  usePinSavedView,
  useReorderSavedViews,
} from "@/lib/api/mutations";
import { useSavedViews } from "@/lib/api/queries";
import { cn } from "@/lib/utils";
import type { SavedViewView } from "@/lib/api/types";

const PIN_CAP = 12;

/** "Home rails" management — the pinned saved views that drive the
 *  rails on `/`. Independent of the sidebar layout: pinning is a flag
 *  on `user_view_pins`, not a sidebar override row. Reusing the same
 *  endpoints the existing rail-management surfaces use keeps the data
 *  model coherent across pages. */
export function HomeRailsSection() {
  const viewsQ = useSavedViews();
  const reorder = useReorderSavedViews();
  const [optimisticOrder, setOptimisticOrder] = React.useState<string[] | null>(
    null,
  );

  const all = React.useMemo(
    () => viewsQ.data?.items ?? [],
    [viewsQ.data?.items],
  );
  const pinned = React.useMemo(() => all.filter((v) => v.pinned), [all]);
  const unpinned = React.useMemo(() => all.filter((v) => !v.pinned), [all]);

  const pinnedIds = React.useMemo(() => pinned.map((v) => v.id), [pinned]);
  const renderIds = optimisticOrder ?? pinnedIds;
  const pinnedById = React.useMemo(() => {
    const m = new Map<string, SavedViewView>();
    for (const v of pinned) m.set(v.id, v);
    return m;
  }, [pinned]);
  const ordered = renderIds
    .map((id) => pinnedById.get(id))
    .filter((v): v is SavedViewView => v !== undefined);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  /** Touch-friendly reorder for viewports where the drag handle is
   *  hidden. Shifts the row by ±1 in the optimistic order, then PATCHes
   *  the same `view_ids` array `reorder` already accepts. */
  function move(viewId: string, direction: -1 | 1) {
    const idx = renderIds.indexOf(viewId);
    const swapIdx = idx + direction;
    if (idx < 0 || swapIdx < 0 || swapIdx >= renderIds.length) return;
    const next = renderIds.slice();
    [next[idx], next[swapIdx]] = [next[swapIdx]!, next[idx]!];
    setOptimisticOrder(next);
    reorder.mutate(
      { view_ids: next },
      {
        onError: () => {
          setOptimisticOrder(null);
          toast.error("Couldn't save the new order");
        },
        onSettled: () => setOptimisticOrder(null),
      },
    );
  }

  function handleDragEnd(ev: DragEndEvent) {
    const { active, over } = ev;
    if (!over || active.id === over.id) return;
    const oldIdx = renderIds.indexOf(String(active.id));
    const newIdx = renderIds.indexOf(String(over.id));
    if (oldIdx < 0 || newIdx < 0) return;
    const next = arrayMove(renderIds, oldIdx, newIdx);
    setOptimisticOrder(next);
    reorder.mutate(
      { view_ids: next },
      {
        onError: () => {
          setOptimisticOrder(null);
          toast.error("Couldn't save the new order");
        },
        onSettled: () => setOptimisticOrder(null),
      },
    );
  }

  const atCap = pinned.length >= PIN_CAP;

  return (
    <section className="flex flex-col gap-3">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold tracking-tight">
            Home rails
          </h2>
          <p className="text-muted-foreground text-sm">
            Drag to reorder the rails that appear on your home page. Up to{" "}
            {PIN_CAP} pins per user ({pinned.length}/{PIN_CAP}).
          </p>
        </div>
        <AddToHomeDialog unpinned={unpinned} atCap={atCap} />
      </header>

      {viewsQ.isLoading ? (
        <div className="text-muted-foreground py-6 text-sm">Loading…</div>
      ) : pinned.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-md border border-dashed p-4 text-sm">
          Nothing pinned yet. Click <strong>Add to home</strong> to pick a
          saved view.
        </div>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          <SortableContext
            items={renderIds}
            strategy={verticalListSortingStrategy}
          >
            <ul className="border-border/60 divide-border/60 divide-y rounded-lg border">
              {ordered.map((view, idx) => (
                <HomeRailRow
                  key={view.id}
                  view={view}
                  onMoveUp={idx > 0 ? () => move(view.id, -1) : undefined}
                  onMoveDown={
                    idx < ordered.length - 1
                      ? () => move(view.id, 1)
                      : undefined
                  }
                />
              ))}
            </ul>
          </SortableContext>
        </DndContext>
      )}
    </section>
  );
}

function HomeRailRow({
  view,
  onMoveUp,
  onMoveDown,
}: {
  view: SavedViewView;
  onMoveUp?: () => void;
  onMoveDown?: () => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: view.id });
  const pin = usePinSavedView();
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };
  const Icon = railIconByKey(view.icon)?.Icon ?? Sparkles;

  return (
    <li
      ref={setNodeRef}
      style={style}
      className={cn(
        "bg-background flex items-center gap-3 px-3 py-2",
        isDragging && "opacity-60",
      )}
    >
      <div className="flex shrink-0 flex-col gap-0.5 sm:hidden">
        <button
          type="button"
          onClick={onMoveUp}
          disabled={!onMoveUp}
          aria-label="Move up"
          className="text-muted-foreground hover:text-foreground disabled:opacity-30 flex h-4 w-7 items-center justify-center rounded-sm"
        >
          <ChevronUp className="h-3 w-3" />
        </button>
        <button
          type="button"
          onClick={onMoveDown}
          disabled={!onMoveDown}
          aria-label="Move down"
          className="text-muted-foreground hover:text-foreground disabled:opacity-30 flex h-4 w-7 items-center justify-center rounded-sm"
        >
          <ChevronDown className="h-3 w-3" />
        </button>
      </div>
      <button
        type="button"
        aria-label="Drag to reorder"
        className="text-muted-foreground hover:text-foreground hidden h-7 w-7 shrink-0 cursor-grab items-center justify-center rounded-md transition-colors active:cursor-grabbing sm:flex"
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(attributes as any)}
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(listeners as any)}
      >
        <GripVertical className="h-4 w-4" />
      </button>
      <Icon className="text-muted-foreground h-4 w-4 shrink-0" />
      <span
        className="min-w-0 flex-1 truncate text-sm font-medium"
        title={view.name}
      >
        {view.name}
      </span>
      {view.is_system ? (
        <Badge
          variant="outline"
          className="text-muted-foreground hidden shrink-0 text-[10px] font-medium uppercase tracking-wider sm:inline-flex"
        >
          Built-in
        </Badge>
      ) : null}
      <Button
        type="button"
        variant="ghost"
        size="sm"
        onClick={() => pin.mutate({ id: view.id, pinned: false })}
        title="Remove from home"
      >
        <X className="mr-1 h-4 w-4" />
        Remove
      </Button>
    </li>
  );
}

// ──────────────────── Add-to-home picker ────────────────────

function AddToHomeDialog({
  unpinned,
  atCap,
}: {
  unpinned: SavedViewView[];
  atCap: boolean;
}) {
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const pin = usePinSavedView();

  const filtered = React.useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return unpinned;
    return unpinned.filter((v) => v.name.toLowerCase().includes(q));
  }, [unpinned, query]);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button type="button" disabled={atCap} title={atCap ? `Pin cap reached (${PIN_CAP})` : undefined}>
          <Plus className="mr-1 h-4 w-4" />
          Add to home
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Add a saved view to home</DialogTitle>
          <DialogDescription>
            Pick a view to pin as a rail on your home page. Manage the
            underlying views from{" "}
            <a
              href="/settings/views"
              className="text-foreground underline underline-offset-2"
            >
              Saved views
            </a>
            .
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter…"
            className="border-border focus:ring-ring rounded-md border px-3 py-1.5 text-sm focus:outline-none focus:ring-2"
            aria-label="Filter saved views"
            autoFocus
          />
          {filtered.length === 0 ? (
            <div className="text-muted-foreground rounded-md border border-dashed py-6 text-center text-sm">
              {unpinned.length === 0
                ? "All saved views are already pinned."
                : "No views match that filter."}
            </div>
          ) : (
            <ul className="border-border/60 divide-border/60 max-h-80 divide-y overflow-y-auto rounded-md border">
              {filtered.map((v) => (
                <li
                  key={v.id}
                  className="flex items-center gap-3 px-3 py-2 text-sm"
                >
                  <span
                    className="min-w-0 flex-1 truncate"
                    title={v.name}
                  >
                    {v.name}
                  </span>
                  {v.is_system ? (
                    <Badge
                      variant="outline"
                      className="text-muted-foreground shrink-0 text-[10px] font-medium uppercase tracking-wider"
                    >
                      Built-in
                    </Badge>
                  ) : null}
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      pin.mutate(
                        { id: v.id, pinned: true },
                        {
                          onSuccess: () => setOpen(false),
                        },
                      );
                    }}
                  >
                    <Plus className="mr-1 h-4 w-4" />
                    Pin
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => setOpen(false)}
          >
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
