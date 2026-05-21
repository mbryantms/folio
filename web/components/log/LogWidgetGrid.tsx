"use client";

import * as React from "react";
import {
  DndContext,
  type DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  rectSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import { useReorderLogWidgets } from "@/lib/api/mutations";
import type { LogWidgetKind, LogWidgetView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

import { WIDGET_REGISTRY } from "./widgets";
import type { LogScope } from "./widgets/types";

/** Drag plumbing each WidgetCard reads via context. The renderer
 *  registry components don't accept drag-handle props directly —
 *  they don't (and shouldn't) know they live inside a sortable —
 *  so we hand off via a per-card Provider in `<SortableWidget>`
 *  and the descendant WidgetCard pulls from context. */
type DragInfo = {
  dragHandleProps: React.HTMLAttributes<HTMLButtonElement>;
  isDragging: boolean;
};
const DragInfoContext = React.createContext<DragInfo | null>(null);

/** Reads the drag info attached by the enclosing `<SortableWidget>`.
 *  Returns `null` when the widget renders outside a sortable
 *  context (e.g., a future read-only embed). */
export function useDragInfo(): DragInfo | null {
  return React.useContext(DragInfoContext);
}

/** The customizable widget grid. Wraps the registry-driven renderer
 *  output in a `@dnd-kit` sortable so the user can drag widgets to
 *  reorder them; the new sequence persists via the reorder mutation.
 *
 *  Optimistic order keeps the visual stable across the round-trip:
 *  local state takes over during the mutation and is cleared on
 *  settle so the next server fetch is the source of truth again.
 *  `rectSortingStrategy` handles the 2-column CSS grid layout
 *  (vertical-list strategy would only work for a single column). */
export function LogWidgetGrid({
  widgets,
  scope,
}: {
  widgets: LogWidgetView[];
  scope: LogScope;
}) {
  const reorder = useReorderLogWidgets();
  const [optimistic, setOptimistic] = React.useState<string[] | null>(null);

  const widgetById = React.useMemo(() => {
    const m = new Map<string, LogWidgetView>();
    for (const w of widgets) m.set(w.id, w);
    return m;
  }, [widgets]);

  const ids: string[] =
    optimistic ??
    widgets
      .slice()
      .sort((a, b) => a.position - b.position)
      .map((w) => w.id);

  const sensors = useSensors(
    // 4px activation distance prevents accidental drags on every
    // click of the card surface.
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const handleDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const oldIdx = ids.indexOf(String(active.id));
    const newIdx = ids.indexOf(String(over.id));
    if (oldIdx < 0 || newIdx < 0) return;
    const next = arrayMove(ids, oldIdx, newIdx);
    setOptimistic(next);
    reorder.mutate(
      { ids: next },
      {
        onError: () => setOptimistic(null),
        onSettled: () => setOptimistic(null),
      },
    );
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <SortableContext items={ids} strategy={rectSortingStrategy}>
        <div className="grid grid-cols-1 gap-6 md:grid-cols-2">
          {ids.map((id) => {
            const w = widgetById.get(id);
            if (!w) return null;
            return <SortableWidget key={id} widget={w} scope={scope} />;
          })}
        </div>
      </SortableContext>
    </DndContext>
  );
}

function SortableWidget({
  widget,
  scope,
}: {
  widget: LogWidgetView;
  scope: LogScope;
}) {
  const def = WIDGET_REGISTRY[widget.kind as LogWidgetKind];
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: widget.id });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  const dragInfo = React.useMemo<DragInfo>(
    () => ({
      dragHandleProps: {
        ref: setActivatorNodeRef as unknown as React.Ref<HTMLButtonElement>,
        ...attributes,
        ...listeners,
      } as React.HTMLAttributes<HTMLButtonElement>,
      isDragging,
    }),
    [setActivatorNodeRef, attributes, listeners, isDragging],
  );

  if (!def) {
    return (
      <div
        ref={setNodeRef}
        style={style}
        className="border-border/60 text-muted-foreground rounded-md border border-dashed p-4 text-sm"
      >
        Unknown widget kind: <code>{widget.kind}</code>
      </div>
    );
  }

  const { Component, size } = def;
  return (
    <div
      ref={setNodeRef}
      style={style}
      className={cn(size === "full" && "md:col-span-2")}
    >
      <DragInfoContext.Provider value={dragInfo}>
        <Component
          widget={widget as LogWidgetView & { config: Record<string, unknown> }}
          scope={scope}
        />
      </DragInfoContext.Provider>
    </div>
  );
}
