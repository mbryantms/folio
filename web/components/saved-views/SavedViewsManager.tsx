"use client";

import * as React from "react";
import Link from "next/link";
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
  GripVertical,
  Lock,
  PanelLeft,
  PanelLeftClose,
  PinOff,
  Pin,
  Trash2,
} from "lucide-react";
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
import {
  useDeleteSavedView,
  usePinSavedView,
  useReorderSavedViews,
  useSidebarSavedView,
} from "@/lib/api/mutations";
import { useSavedViews } from "@/lib/api/queries";
import { cn } from "@/lib/utils";
import type { SavedViewView } from "@/lib/api/types";

import { AddViewButton } from "./AddViewButton";

const PIN_CAP = 12;

/** Per-user saved-views management surface. Lives at
 *  `/settings/views`. Pinned views can be drag-reordered; the user
 *  can pin/unpin, edit (via /views/{id}), or delete. */
export function SavedViewsManager() {
  const viewsQ = useSavedViews();
  const reorder = useReorderSavedViews();
  const [optimisticOrder, setOptimisticOrder] = React.useState<string[] | null>(
    null,
  );

  const all = viewsQ.data?.items ?? [];
  const pinned = all.filter((v) => v.pinned);
  const unpinned = all.filter((v) => !v.pinned);

  // Pin order from server vs the locally-optimistic post-drop order.
  const pinnedIds = React.useMemo(() => pinned.map((v) => v.id), [pinned]);
  const renderPinnedIds = optimisticOrder ?? pinnedIds;
  const pinnedById = React.useMemo(() => {
    const m = new Map<string, SavedViewView>();
    for (const v of pinned) m.set(v.id, v);
    return m;
  }, [pinned]);
  const orderedPinned = renderPinnedIds
    .map((id) => pinnedById.get(id))
    .filter((v): v is SavedViewView => v !== undefined);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  function handleDragEnd(ev: DragEndEvent) {
    const { active, over } = ev;
    if (!over || active.id === over.id) return;
    const oldIndex = renderPinnedIds.indexOf(String(active.id));
    const newIndex = renderPinnedIds.indexOf(String(over.id));
    if (oldIndex < 0 || newIndex < 0) return;
    const next = arrayMove(renderPinnedIds, oldIndex, newIndex);
    setOptimisticOrder(next);
    reorder.mutate(
      { view_ids: next },
      {
        onError: () => {
          setOptimisticOrder(null);
          toast.error("Couldn't save the new order");
        },
      },
    );
  }

  const atPinCap = pinned.length >= PIN_CAP;

  if (viewsQ.isLoading) {
    return (
      <div className="text-muted-foreground py-12 text-sm">Loading views…</div>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <AddViewButton />
      </div>

      <Section
        title="Pinned"
        description={`Drag to reorder. Up to ${PIN_CAP} pins per user (${pinned.length}/${PIN_CAP}).`}
      >
        {pinned.length === 0 ? (
          <EmptyHint message="Nothing pinned yet. Pin a view below to make it show up on the home page." />
        ) : (
          <DndContext
            sensors={sensors}
            collisionDetection={closestCenter}
            onDragEnd={handleDragEnd}
          >
            <SortableContext
              items={renderPinnedIds}
              strategy={verticalListSortingStrategy}
            >
              <ul className="border-border/60 divide-border/60 divide-y rounded-lg border">
                {orderedPinned.map((view) => (
                  <SortableRow key={view.id} view={view} />
                ))}
              </ul>
            </SortableContext>
          </DndContext>
        )}
      </Section>

      <Section
        title="All views"
        description="System and personal views available to pin."
      >
        {unpinned.length === 0 ? (
          <EmptyHint message="No unpinned views." />
        ) : (
          <ul className="border-border/60 divide-border/60 divide-y rounded-lg border">
            {unpinned.map((view) => (
              <ViewRow key={view.id} view={view} atPinCap={atPinCap} />
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}

function Section({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="text-base font-semibold tracking-tight">{title}</h2>
        {description ? (
          <p className="text-muted-foreground text-sm">{description}</p>
        ) : null}
      </div>
      {children}
    </section>
  );
}

function EmptyHint({ message }: { message: string }) {
  return (
    <div className="border-border/60 text-muted-foreground rounded-md border border-dashed p-4 text-sm">
      {message}
    </div>
  );
}

function SortableRow({ view }: { view: SavedViewView }) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: view.id });
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };
  return (
    <ViewRow
      view={view}
      innerRef={setNodeRef}
      style={style}
      isDragging={isDragging}
      dragHandle={{
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        attributes: attributes as any,
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        listeners: listeners as any,
      }}
    />
  );
}

function ViewRow({
  view,
  atPinCap = false,
  dragHandle,
  innerRef,
  style,
  isDragging,
}: {
  view: SavedViewView;
  atPinCap?: boolean;
  dragHandle?: {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    attributes?: Record<string, any>;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    listeners?: Record<string, any>;
  };
  innerRef?: (node: HTMLElement | null) => void;
  style?: React.CSSProperties;
  isDragging?: boolean;
}) {
  const pin = usePinSavedView();
  const sidebar = useSidebarSavedView();
  const del = useDeleteSavedView(view.id);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const isCbl = view.kind === "cbl";

  return (
    <li
      ref={innerRef as React.Ref<HTMLLIElement>}
      style={style}
      className={cn(
        "bg-background flex items-center gap-3 px-3 py-2",
        isDragging && "opacity-60",
      )}
    >
      {dragHandle ? (
        <button
          type="button"
          aria-label="Drag to reorder"
          className="text-muted-foreground hover:text-foreground hidden h-7 w-7 shrink-0 cursor-grab items-center justify-center rounded-md transition-colors active:cursor-grabbing sm:flex"
          {...(dragHandle.attributes ?? {})}
          {...(dragHandle.listeners ?? {})}
        >
          <GripVertical className="h-4 w-4" />
        </button>
      ) : (
        <span aria-hidden className="hidden w-7 sm:block" />
      )}

      <div className="flex min-w-0 flex-1 items-center gap-2">
        <Link
          href={`/views/${view.id}`}
          className="hover:text-foreground truncate text-sm font-medium"
          title={view.name}
        >
          {view.name}
        </Link>
        {view.is_system ? (
          <span
            className="text-muted-foreground bg-muted/40 inline-flex shrink-0 items-center rounded-md border px-2 py-0.5 text-xs"
            title="Built-in views can't be edited or deleted"
          >
            <Lock className="mr-1 h-3 w-3" /> Built-in
          </span>
        ) : null}
      </div>

      <div className="flex shrink-0 items-center gap-1">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => pin.mutate({ id: view.id, pinned: !view.pinned })}
          disabled={!view.pinned && atPinCap}
          title={
            !view.pinned && atPinCap
              ? `Pin cap reached (${PIN_CAP}). Unpin one to add another.`
              : view.pinned
                ? "Unpin from home"
                : "Pin to home"
          }
        >
          {view.pinned ? (
            <>
              <PinOff className="mr-1 h-4 w-4" />
              Unpin
            </>
          ) : (
            <>
              <Pin className="mr-1 h-4 w-4" />
              Pin
            </>
          )}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() =>
            sidebar.mutate({ id: view.id, show: !view.show_in_sidebar })
          }
          title={view.show_in_sidebar ? "Hide from sidebar" : "Show in sidebar"}
        >
          {view.show_in_sidebar ? (
            <>
              <PanelLeftClose className="mr-1 h-4 w-4" />
              Hide
            </>
          ) : (
            <>
              <PanelLeft className="mr-1 h-4 w-4" />
              Sidebar
            </>
          )}
        </Button>
        {view.is_system ? null : (
          <>
            <Button type="button" variant="ghost" size="sm" asChild>
              <Link href={`/views/${view.id}`}>Edit</Link>
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              onClick={() => setConfirmOpen(true)}
              aria-label="Delete view"
              className="text-destructive hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" />
            </Button>
            <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>Delete this view?</AlertDialogTitle>
                  <AlertDialogDescription>
                    {isCbl
                      ? "Removes the saved view but keeps the underlying CBL list. You can re-import or re-pin later."
                      : "Removes the filter view permanently."}
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>Cancel</AlertDialogCancel>
                  <AlertDialogAction onClick={() => del.mutate()}>
                    Delete
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          </>
        )}
      </div>
    </li>
  );
}
