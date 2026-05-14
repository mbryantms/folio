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
  RotateCcw,
  Sparkles,
} from "lucide-react";
import { toast } from "sonner";

import { mainNavIcons } from "@/components/library/main-nav-icons";
import { railIconByKey } from "@/components/library/rail-icons";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { useSidebarLayout } from "@/lib/api/queries";
import { useUpdateSidebarLayout } from "@/lib/api/mutations";
import { cn } from "@/lib/utils";
import type {
  SidebarEntryView,
  SidebarLayoutView,
  UpdateEntryReq,
} from "@/lib/api/types";

import { HomeRailsSection } from "./HomeRailsSection";

/** Top-level manager for `/settings/navigation`. Two independent
 *  sections share the page:
 *
 *    1. **Home rails** — the saved-view pin set (reorder + remove + add)
 *       that drives the home page rails. Reuses the existing pin/reorder
 *       mutations because the pin set is per-saved-view state, not
 *       part of the sidebar layout.
 *
 *    2. **Sidebar** — a unified ordered list of everything in the left
 *       nav (built-ins, libraries, saved views). Drag to reorder; toggle
 *       to hide. "Reset to defaults" wipes overrides. */
export function NavigationManager() {
  return (
    <div className="flex flex-col gap-10">
      <HomeRailsSection />
      <SidebarSection />
    </div>
  );
}

// ──────────────────── Sidebar section ────────────────────

function SidebarSection() {
  const layoutQ = useSidebarLayout();
  const update = useUpdateSidebarLayout();
  // Optimistic post-drag order. Mirrors `SavedViewsManager`'s pattern:
  // local state wins until the server reconciles, at which point we
  // drop back to the query's resolved layout.
  const [optimistic, setOptimistic] = React.useState<SidebarEntryView[] | null>(
    null,
  );

  const layout = layoutQ.data;
  const entries = React.useMemo(
    () => optimistic ?? layout?.entries ?? [],
    [optimistic, layout],
  );

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  function toPayload(items: SidebarEntryView[]): UpdateEntryReq[] {
    // Reassign positions to a dense 0..N range so server-side row
    // positions stay tidy and predictable; the input array order is
    // the source of truth.
    return items.map((e, idx) => ({
      kind: e.kind,
      ref_id: e.ref_id,
      visible: e.visible,
      position: idx,
    }));
  }

  function handleDragEnd(ev: DragEndEvent) {
    const { active, over } = ev;
    if (!over || active.id === over.id) return;
    const keyOf = (e: SidebarEntryView) => `${e.kind}:${e.ref_id}`;
    const ids = entries.map(keyOf);
    const oldIdx = ids.indexOf(String(active.id));
    const newIdx = ids.indexOf(String(over.id));
    if (oldIdx < 0 || newIdx < 0) return;
    const next = arrayMove(entries, oldIdx, newIdx);
    setOptimistic(next);
    update.mutate(
      { entries: toPayload(next) },
      {
        onError: () => {
          setOptimistic(null);
          // toast handled by useApiMutation
        },
        onSettled: () => setOptimistic(null),
      },
    );
  }

  /** Touch-friendly reorder for viewports where the drag handle is
   *  hidden. Shifts the entry by ±1 in the visible order, reusing the
   *  same optimistic+PATCH path as drag-end. `direction = -1` moves
   *  up; `+1` moves down. */
  function move(target: SidebarEntryView, direction: -1 | 1) {
    const idx = entries.findIndex(
      (e) => e.kind === target.kind && e.ref_id === target.ref_id,
    );
    const swapIdx = idx + direction;
    if (idx < 0 || swapIdx < 0 || swapIdx >= entries.length) return;
    const next = entries.slice();
    [next[idx], next[swapIdx]] = [next[swapIdx]!, next[idx]!];
    setOptimistic(next);
    update.mutate(
      { entries: toPayload(next) },
      {
        onError: () => setOptimistic(null),
        onSettled: () => setOptimistic(null),
      },
    );
  }

  function toggleVisible(target: SidebarEntryView) {
    // Whole-array replace: build the payload from the current entries
    // with just the target flipped. Other rows pass through unchanged
    // so they keep their existing visibility/position.
    const next = entries.map((e) =>
      e.kind === target.kind && e.ref_id === target.ref_id
        ? { ...e, visible: !e.visible }
        : e,
    );
    setOptimistic(next);
    update.mutate(
      { entries: toPayload(next) },
      {
        onError: () => setOptimistic(null),
        onSettled: () => setOptimistic(null),
      },
    );
  }

  function resetToDefaults() {
    setOptimistic(null);
    update.mutate(
      { entries: [] },
      {
        onSuccess: () => toast.success("Sidebar reset to defaults"),
      },
    );
  }

  if (layoutQ.isLoading) {
    return (
      <Section
        title="Sidebar"
        description="Loading layout…"
        loading
      />
    );
  }
  if (layoutQ.isError || !layout) {
    return (
      <Section
        title="Sidebar"
        description="Failed to load sidebar layout."
      />
    );
  }

  const hasAnyHidden = entries.some((e) => !e.visible);

  return (
    <Section
      title="Sidebar"
      description="Drag rows to reorder. Toggle to hide an entry without deleting it."
      action={
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button variant="outline" size="sm" type="button">
              <RotateCcw className="mr-1.5 h-4 w-4" />
              Reset to defaults
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Reset sidebar to defaults?</AlertDialogTitle>
              <AlertDialogDescription>
                Every customization here — hidden rows, reorders — gets
                cleared. New libraries and saved views you add later will
                automatically appear.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={resetToDefaults}>
                Reset
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      }
    >
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext
          items={entries.map((e) => `${e.kind}:${e.ref_id}`)}
          strategy={verticalListSortingStrategy}
        >
          <ul className="border-border/60 divide-border/60 divide-y rounded-lg border">
            {entries.map((entry, idx) => (
              <SidebarRow
                key={`${entry.kind}:${entry.ref_id}`}
                entry={entry}
                onToggleVisible={() => toggleVisible(entry)}
                onMoveUp={
                  idx > 0 ? () => move(entry, -1) : undefined
                }
                onMoveDown={
                  idx < entries.length - 1
                    ? () => move(entry, 1)
                    : undefined
                }
              />
            ))}
          </ul>
        </SortableContext>
      </DndContext>
      {hasAnyHidden ? (
        <p className="text-muted-foreground text-xs">
          Hidden rows stay in this list so you can bring them back later.
        </p>
      ) : null}
    </Section>
  );
}

/** Stacked up/down chevrons used as a touch-friendly reorder
 *  affordance on mobile, where the drag handle is hidden. Each
 *  button reduces to a no-op (disabled) at the boundary so the
 *  layout stays stable as the row moves through positions. */
function MobileMoveButtons({
  onUp,
  onDown,
}: {
  onUp?: () => void;
  onDown?: () => void;
}) {
  return (
    <div className="flex shrink-0 flex-col gap-0.5 sm:hidden">
      <button
        type="button"
        onClick={onUp}
        disabled={!onUp}
        aria-label="Move up"
        className="text-muted-foreground hover:text-foreground disabled:opacity-30 flex h-4 w-7 items-center justify-center rounded-sm"
      >
        <ChevronUp className="h-3 w-3" />
      </button>
      <button
        type="button"
        onClick={onDown}
        disabled={!onDown}
        aria-label="Move down"
        className="text-muted-foreground hover:text-foreground disabled:opacity-30 flex h-4 w-7 items-center justify-center rounded-sm"
      >
        <ChevronDown className="h-3 w-3" />
      </button>
    </div>
  );
}

function SidebarRow({
  entry,
  onToggleVisible,
  onMoveUp,
  onMoveDown,
}: {
  entry: SidebarEntryView;
  onToggleVisible: () => void;
  /** Mobile-only reorder. `undefined` at the boundary row (first/last)
   *  so the button renders disabled. */
  onMoveUp?: () => void;
  onMoveDown?: () => void;
}) {
  const id = `${entry.kind}:${entry.ref_id}`;
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id });
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
  };
  // Resolution order mirrors `MainSidebar.tsx` so the icon shown here
  // matches what the user sees in the actual nav.
  const Icon =
    mainNavIcons[entry.icon as keyof typeof mainNavIcons] ??
    railIconByKey(entry.icon)?.Icon ??
    Sparkles;

  return (
    <li
      ref={setNodeRef}
      style={style}
      className={cn(
        "bg-background flex items-center gap-3 px-3 py-2",
        isDragging && "opacity-60",
        !entry.visible && "opacity-50",
      )}
    >
      <MobileMoveButtons onUp={onMoveUp} onDown={onMoveDown} />
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
      <span className="min-w-0 flex-1 truncate text-sm">{entry.label}</span>
      <KindChip kind={entry.kind} />
      <Switch
        checked={entry.visible}
        onCheckedChange={onToggleVisible}
        aria-label={entry.visible ? "Hide from sidebar" : "Show in sidebar"}
      />
    </li>
  );
}

function KindChip({ kind }: { kind: SidebarEntryView["kind"] }) {
  const label =
    kind === "builtin" ? "Built-in" : kind === "library" ? "Library" : "View";
  return (
    <Badge
      variant="outline"
      className="text-muted-foreground hidden shrink-0 text-[10px] font-medium uppercase tracking-wider sm:inline-flex"
    >
      {label}
    </Badge>
  );
}

// ──────────────────── Section primitive ────────────────────

function Section({
  title,
  description,
  action,
  children,
  loading,
}: {
  title: string;
  description?: string;
  action?: React.ReactNode;
  children?: React.ReactNode;
  loading?: boolean;
}) {
  return (
    <section className="flex flex-col gap-3">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-base font-semibold tracking-tight">{title}</h2>
          {description ? (
            <p className="text-muted-foreground text-sm">{description}</p>
          ) : null}
        </div>
        {action}
      </header>
      {loading ? (
        <div className="text-muted-foreground py-6 text-sm">Loading…</div>
      ) : (
        children
      )}
    </section>
  );
}

// Re-exported here so co-located callers can stick to one import path.
// (The actual file does the heavy lifting.)
export type { SidebarLayoutView };
