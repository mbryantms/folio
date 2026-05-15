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
  Heading2,
  Pencil,
  Plus,
  RotateCcw,
  Rows3,
  Search,
  Sparkles,
  Trash2,
  X,
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
import { Input } from "@/components/ui/input";
import { useSidebarLayout } from "@/lib/api/queries";
import { useUpdateSidebarLayout } from "@/lib/api/mutations";
import { cn } from "@/lib/utils";
import type {
  SidebarEntryView,
  SidebarLayoutView,
  UpdateEntryReq,
} from "@/lib/api/types";

/** Top-level manager for `/settings/navigation`. Owns the unified
 *  ordered sidebar list — built-ins, libraries, pages, saved views,
 *  plus user-inserted headers and spacers. Home rails (the per-page
 *  rail pin set) moved to `/settings/pages`. */
export function NavigationManager() {
  return <SidebarSection />;
}

/** Web doesn't have access to `crypto.randomUUID()` under the SSR
 *  build target, but this component is client-only (`"use client"`)
 *  so the helper is safe. Used to mint stable ref_ids for new
 *  header/spacer rows the user inserts. */
function newRefId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  // Fallback for older Node test env (vitest under bare node). Format
  // matches a v4 UUID enough for the server's TEXT ref_id column.
  return "00000000-0000-4000-8000-" + Math.random().toString(16).slice(2, 14);
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
  // Settings list shows ONLY the rows the user has chosen to keep in
  // the sidebar. Hidden rows are preserved server-side (so the Add
  // picker can re-show them) but stay out of sight here.
  const visibleEntries = React.useMemo(
    () => entries.filter((e) => e.visible),
    [entries],
  );
  const hiddenEntries = React.useMemo(
    () => entries.filter((e) => !e.visible),
    [entries],
  );
  const [addOpen, setAddOpen] = React.useState(false);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  function toPayload(items: SidebarEntryView[]): UpdateEntryReq[] {
    // Reassign positions to a dense 0..N range so server-side row
    // positions stay tidy and predictable; the input array order is
    // the source of truth. Forward the label too so renames + custom
    // header text survive the wholesale replace the server does.
    return items.map((e, idx) => ({
      kind: e.kind,
      ref_id: e.ref_id,
      visible: e.visible,
      position: idx,
      label: e.kind === "header" || e.label ? e.label : null,
    }));
  }

  /** Combine the new visible-order with the user's hidden rows so the
   *  PATCH payload preserves both. Hidden rows trail the visible ones;
   *  position values are reassigned densely. Without this, the server's
   *  whole-array replace would drop the visible=false overrides and
   *  every previously-hidden default would re-appear on the next GET. */
  function buildPayload(visibleNext: SidebarEntryView[]): UpdateEntryReq[] {
    return toPayload([...visibleNext, ...hiddenEntries]);
  }

  function handleDragEnd(ev: DragEndEvent) {
    const { active, over } = ev;
    if (!over || active.id === over.id) return;
    const keyOf = (e: SidebarEntryView) => `${e.kind}:${e.ref_id}`;
    const ids = visibleEntries.map(keyOf);
    const oldIdx = ids.indexOf(String(active.id));
    const newIdx = ids.indexOf(String(over.id));
    if (oldIdx < 0 || newIdx < 0) return;
    const nextVisible = arrayMove(visibleEntries, oldIdx, newIdx);
    const next = [...nextVisible, ...hiddenEntries];
    setOptimistic(next);
    update.mutate(
      { entries: buildPayload(nextVisible) },
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
   *  hidden. Shifts the entry by ±1 within the visible-row order. */
  function move(target: SidebarEntryView, direction: -1 | 1) {
    const idx = visibleEntries.findIndex(
      (e) => e.kind === target.kind && e.ref_id === target.ref_id,
    );
    const swapIdx = idx + direction;
    if (idx < 0 || swapIdx < 0 || swapIdx >= visibleEntries.length) return;
    const nextVisible = visibleEntries.slice();
    [nextVisible[idx], nextVisible[swapIdx]] = [
      nextVisible[swapIdx]!,
      nextVisible[idx]!,
    ];
    const next = [...nextVisible, ...hiddenEntries];
    setOptimistic(next);
    update.mutate(
      { entries: buildPayload(nextVisible) },
      {
        onError: () => setOptimistic(null),
        onSettled: () => setOptimistic(null),
      },
    );
  }

  /** Hide a row from the sidebar. The override row stays in storage so
   *  the user can re-add the entry from the picker. */
  function hideRow(target: SidebarEntryView) {
    const nextVisible = visibleEntries.filter(
      (e) => !(e.kind === target.kind && e.ref_id === target.ref_id),
    );
    const hidden = { ...target, visible: false };
    const next = [...nextVisible, ...hiddenEntries, hidden];
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

  function commitMutation(nextVisible: SidebarEntryView[]) {
    setOptimistic([...nextVisible, ...hiddenEntries]);
    update.mutate(
      { entries: buildPayload(nextVisible) },
      {
        onError: () => setOptimistic(null),
        onSettled: () => setOptimistic(null),
      },
    );
  }

  function appendHeader() {
    const nextVisible = [
      ...visibleEntries,
      {
        kind: "header" as const,
        ref_id: newRefId(),
        label: "New header",
        icon: "",
        href: "",
        visible: true,
        position: visibleEntries.length,
      },
    ];
    commitMutation(nextVisible);
  }

  function appendSpacer() {
    const nextVisible = [
      ...visibleEntries,
      {
        kind: "spacer" as const,
        ref_id: newRefId(),
        label: "",
        icon: "",
        href: "",
        visible: true,
        position: visibleEntries.length,
      },
    ];
    commitMutation(nextVisible);
  }

  function renameLabel(target: SidebarEntryView, label: string) {
    const trimmed = label.trim();
    if (trimmed.length === 0) return;
    const nextVisible = visibleEntries.map((e) =>
      e.kind === target.kind && e.ref_id === target.ref_id
        ? { ...e, label: trimmed }
        : e,
    );
    commitMutation(nextVisible);
  }

  /** Drop a custom header/spacer entirely. Default rows can't be
   *  removed — hideRow handles those by storing a visible=false
   *  override so they can be re-added from the picker. */
  function removeRow(target: SidebarEntryView) {
    if (target.kind !== "header" && target.kind !== "spacer") return;
    const nextVisible = visibleEntries.filter(
      (e) => !(e.kind === target.kind && e.ref_id === target.ref_id),
    );
    commitMutation(nextVisible);
  }

  if (layoutQ.isLoading) {
    return (
      <div className="text-muted-foreground py-6 text-sm">Loading layout…</div>
    );
  }
  if (layoutQ.isError || !layout) {
    return (
      <div className="text-destructive py-6 text-sm">
        Failed to load sidebar layout.
      </div>
    );
  }

  return (
    <section className="flex flex-col gap-3">
      {/* The route's `<PageHeader>` already shows the page title +
       *  description; an inner section header would duplicate it.
       *  Just render the toolbar + list. */}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button
          variant="default"
          size="sm"
          type="button"
          onClick={() => setAddOpen(true)}
        >
          <Plus className="mr-1.5 h-4 w-4" />
          Add
        </Button>
        <Button
          variant="outline"
          size="sm"
          type="button"
          onClick={appendHeader}
        >
          <Heading2 className="mr-1.5 h-4 w-4" />
          Header
        </Button>
        <Button
          variant="outline"
          size="sm"
          type="button"
          onClick={appendSpacer}
        >
          <Rows3 className="mr-1.5 h-4 w-4" />
          Spacer
        </Button>
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button variant="outline" size="sm" type="button">
              <RotateCcw className="mr-1.5 h-4 w-4" />
              Reset
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Reset sidebar to defaults?</AlertDialogTitle>
              <AlertDialogDescription>
                Every customization here — removed rows, reorders, custom
                headers + spacers — gets cleared. New libraries and saved
                views you add later will automatically appear.
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
      </div>
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext
          items={visibleEntries.map((e) => `${e.kind}:${e.ref_id}`)}
          strategy={verticalListSortingStrategy}
        >
          <ul className="border-border/60 divide-border/60 divide-y rounded-lg border">
            {visibleEntries.map((entry, idx) => {
              const customDivider =
                (entry.kind === "header" || entry.kind === "spacer") &&
                !entry.ref_id.startsWith("default:");
              return (
                <SidebarRow
                  key={`${entry.kind}:${entry.ref_id}`}
                  entry={entry}
                  onRename={(label) => renameLabel(entry, label)}
                  onRemove={
                    // Custom dividers: drop them entirely.
                    // Everything else: hide so the user can re-add later.
                    customDivider
                      ? () => removeRow(entry)
                      : () => hideRow(entry)
                  }
                  onMoveUp={idx > 0 ? () => move(entry, -1) : undefined}
                  onMoveDown={
                    idx < visibleEntries.length - 1
                      ? () => move(entry, 1)
                      : undefined
                  }
                />
              );
            })}
          </ul>
        </SortableContext>
      </DndContext>
      <AddToSidebarDialog
        open={addOpen}
        onOpenChange={setAddOpen}
        hiddenEntries={hiddenEntries}
        onShowExisting={(target) => {
          // Move from hidden → visible and append at the bottom of
          // the visible list. The user can drag it where they want.
          const remaining = hiddenEntries.filter(
            (e) => !(e.kind === target.kind && e.ref_id === target.ref_id),
          );
          const restored = { ...target, visible: true };
          const nextVisible = [...visibleEntries, restored];
          const next = [...nextVisible, ...remaining];
          setOptimistic(next);
          update.mutate(
            { entries: toPayload(next) },
            {
              onError: () => setOptimistic(null),
              onSettled: () => setOptimistic(null),
            },
          );
        }}
      />
    </section>
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
  onRename,
  onRemove,
  onMoveUp,
  onMoveDown,
}: {
  entry: SidebarEntryView;
  /** Update this row's label. Used for headers + label overrides on
   *  builtins/libraries/views/pages. */
  onRename: (label: string) => void;
  /** Drop the row from the sidebar. For custom dividers (header/spacer)
   *  this removes the override row entirely; for everything else it
   *  stores a visible=false override so the Add picker can re-add it. */
  onRemove: () => void;
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

  const isHeader = entry.kind === "header";
  const isSpacer = entry.kind === "spacer";
  // Headers + a few default kinds support label rename inline; only
  // headers really need it, but allowing user-defined overrides on the
  // others is cheap and matches the "everything's customizable" spirit.
  const renamable = isHeader || entry.kind === "builtin" || entry.kind === "library" || entry.kind === "view" || entry.kind === "page";
  const [editing, setEditing] = React.useState(false);
  const [draft, setDraft] = React.useState(entry.label);
  const [lastSeen, setLastSeen] = React.useState(entry.label);
  if (lastSeen !== entry.label) {
    setLastSeen(entry.label);
    setDraft(entry.label);
  }

  // Resolution order mirrors `MainSidebar.tsx` so the icon shown here
  // matches what the user sees in the actual nav.
  const Icon =
    mainNavIcons[entry.icon as keyof typeof mainNavIcons] ??
    railIconByKey(entry.icon)?.Icon ??
    Sparkles;

  const commitRename = () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed.length === 0 || trimmed === entry.label) {
      setDraft(entry.label);
      return;
    }
    onRename(trimmed);
  };

  return (
    <li
      ref={setNodeRef}
      style={style}
      className={cn(
        "bg-background flex items-center gap-3 px-3 py-2",
        isDragging && "opacity-60",
        !entry.visible && "opacity-50",
        isHeader && "bg-muted/30",
        isSpacer && "bg-muted/10 py-1",
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
      {isHeader ? (
        <Heading2 className="text-muted-foreground h-4 w-4 shrink-0" />
      ) : isSpacer ? (
        <Rows3 className="text-muted-foreground h-4 w-4 shrink-0" />
      ) : (
        <Icon className="text-muted-foreground h-4 w-4 shrink-0" />
      )}
      {isSpacer ? (
        <span className="text-muted-foreground/70 min-w-0 flex-1 text-xs italic">
          Spacer
        </span>
      ) : editing && renamable ? (
        <Input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commitRename}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitRename();
            } else if (e.key === "Escape") {
              e.preventDefault();
              setDraft(entry.label);
              setEditing(false);
            }
          }}
          className={cn(
            "min-w-0 flex-1 text-sm",
            isHeader && "font-semibold uppercase tracking-wider",
          )}
        />
      ) : (
        <button
          type="button"
          onClick={renamable ? () => setEditing(true) : undefined}
          className={cn(
            "group min-w-0 flex-1 truncate text-left text-sm",
            renamable && "hover:bg-secondary/40 -mx-1 rounded px-1",
            isHeader && "font-semibold uppercase tracking-wider",
          )}
          title={renamable ? "Click to rename" : undefined}
          disabled={!renamable}
        >
          <span className="truncate">{entry.label}</span>
          {renamable ? (
            <Pencil className="text-muted-foreground/0 group-hover:text-muted-foreground ml-1.5 inline-block h-3 w-3 align-middle transition-colors" />
          ) : null}
        </button>
      )}
      <KindChip kind={entry.kind} />
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="text-muted-foreground hover:text-destructive h-7 w-7"
        onClick={onRemove}
        aria-label="Remove from sidebar"
        title="Remove from sidebar"
      >
        {isSpacer ? (
          <X className="h-3.5 w-3.5" />
        ) : (
          <Trash2 className="h-3.5 w-3.5" />
        )}
      </Button>
    </li>
  );
}

function KindChip({ kind }: { kind: SidebarEntryView["kind"] }) {
  const label =
    kind === "builtin"
      ? "Built-in"
      : kind === "library"
        ? "Library"
        : kind === "view"
          ? "View"
          : kind === "page"
            ? "Page"
            : kind === "header"
              ? "Header"
              : "Spacer";
  return (
    <Badge
      variant="outline"
      className="text-muted-foreground hidden shrink-0 text-[10px] font-medium uppercase tracking-wider sm:inline-flex"
    >
      {label}
    </Badge>
  );
}

// ──────────────────── Add to sidebar picker ────────────────────

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useSavedViews } from "@/lib/api/queries";
import { useSidebarSavedView } from "@/lib/api/mutations";

/** Picker for the Sidebar settings page. Surfaces things the user
 *  can bring INTO the sidebar:
 *
 *    1. Previously-hidden rows (built-ins, libraries, pages, custom
 *       headers/spacers, plus saved views the user toggled off). These
 *       have an existing override row with `visible: false`; showing
 *       them flips that flag.
 *    2. Saved views (filter views and CBL lists) that aren't currently
 *       in the sidebar at all — i.e. `show_in_sidebar=false` on their
 *       pin row. Adding flips that flag, which makes `compute_layout`
 *       start emitting them.
 *
 *  System views never appear in this picker (Continue Reading et al.
 *  belong to the home rails, not the sidebar). */
/** Internal row-kind taxonomy for the Add picker. Saved views split
 *  into Filter / CBL / Collection so users can scan the buckets they
 *  expect to find a row in — the underlying server entry is still
 *  `kind='view'` regardless. */
type AddRowKind =
  | "builtin"
  | "library"
  | "page"
  | "filter"
  | "cbl"
  | "collection"
  | "header"
  | "spacer";

/** Group order from top to bottom in the dialog. */
const ADD_GROUP_ORDER: ReadonlyArray<AddRowKind> = [
  "builtin",
  "page",
  "library",
  "filter",
  "cbl",
  "collection",
  "header",
  "spacer",
];

function addRowKindLabel(kind: AddRowKind): string {
  switch (kind) {
    case "builtin":
      return "Built-in";
    case "library":
      return "Library";
    case "page":
      return "Page";
    case "filter":
      return "Filter view";
    case "cbl":
      return "CBL list";
    case "collection":
      return "Collection";
    case "header":
      return "Header";
    case "spacer":
      return "Spacer";
  }
}

function addGroupLabel(kind: AddRowKind): string {
  switch (kind) {
    case "builtin":
      return "Built-ins";
    case "library":
      return "Libraries";
    case "page":
      return "Pages";
    case "filter":
      return "Filter views";
    case "cbl":
      return "CBL lists";
    case "collection":
      return "Collections";
    case "header":
      return "Headers";
    case "spacer":
      return "Spacers";
  }
}

type AddItem = {
  /** Stable key — `${source}:${kind}:${ref_id}` so collisions across
   *  origins (hidden override vs not-yet-shown saved view) are
   *  impossible. */
  id: string;
  label: string;
  description?: string | null;
  kind: AddRowKind;
  source: "hidden" | "saved-view";
  /** Carries the underlying record so the picker can dispatch the
   *  right mutation when the user clicks Add. */
  payload:
    | { source: "hidden"; entry: SidebarEntryView }
    | { source: "saved-view"; viewId: string };
};

function AddToSidebarDialog({
  open,
  onOpenChange,
  hiddenEntries,
  onShowExisting,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  hiddenEntries: SidebarEntryView[];
  /** Re-show a row that's already in the override table with
   *  `visible: false`. The caller handles the PATCH. */
  onShowExisting: (entry: SidebarEntryView) => void;
}) {
  // Pull every saved view so we can spot the ones flagged
  // `show_in_sidebar=false`. Hidden by default for performance — the
  // dialog only opens when the user clicks Add.
  const savedQ = useSavedViews();
  const flipSidebar = useSidebarSavedView();
  const [query, setQuery] = React.useState("");

  // Reset search on (re-)open. Render-phase setState idiom — see
  // https://react.dev/learn/you-might-not-need-an-effect.
  const [lastOpen, setLastOpen] = React.useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) setQuery("");
  }

  const hiddenSavedViewIds = React.useMemo(
    () =>
      new Set(
        hiddenEntries
          .filter((e) => e.kind === "view")
          .map((e) => e.ref_id),
      ),
    [hiddenEntries],
  );

  // Build the full add-item list. Each item carries enough info to
  // route the Add click + place itself in the right group.
  const allItems = React.useMemo<AddItem[]>(() => {
    const items: AddItem[] = [];
    for (const entry of hiddenEntries) {
      const rowKind: AddRowKind =
        entry.kind === "view" ? "filter" : (entry.kind as AddRowKind);
      // For hidden views in the override table we don't have the
      // saved_view kind handy (the layout response collapses cbl/
      // collection into `kind='view'`). Cross-reference saved views
      // for a more precise chip when possible.
      let preciseKind: AddRowKind = rowKind;
      if (entry.kind === "view") {
        const match = savedQ.data?.items.find((v) => v.id === entry.ref_id);
        if (match?.kind === "cbl") preciseKind = "cbl";
        else if (match?.kind === "collection") preciseKind = "collection";
        else if (match?.kind === "filter_series") preciseKind = "filter";
      }
      items.push({
        id: `hidden:${entry.kind}:${entry.ref_id}`,
        label:
          entry.label.length > 0
            ? entry.label
            : entry.kind === "spacer"
              ? "Spacer"
              : entry.kind === "header"
                ? "Header"
                : entry.ref_id,
        kind: preciseKind,
        source: "hidden",
        payload: { source: "hidden", entry },
      });
    }
    const views = savedQ.data?.items ?? [];
    for (const v of views) {
      if (v.is_system || v.show_in_sidebar !== false) continue;
      if (hiddenSavedViewIds.has(v.id)) continue;
      const rowKind: AddRowKind =
        v.kind === "cbl"
          ? "cbl"
          : v.kind === "collection"
            ? "collection"
            : "filter";
      items.push({
        id: `view:${v.id}`,
        label: v.name,
        description: v.description ?? null,
        kind: rowKind,
        source: "saved-view",
        payload: { source: "saved-view", viewId: v.id },
      });
    }
    return items;
  }, [hiddenEntries, savedQ.data?.items, hiddenSavedViewIds]);

  const filtered = React.useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q.length === 0) return allItems;
    return allItems.filter((item) => {
      if (item.label.toLowerCase().includes(q)) return true;
      if (item.description && item.description.toLowerCase().includes(q))
        return true;
      return false;
    });
  }, [allItems, query]);

  const groups = React.useMemo(() => {
    const byKind = new Map<AddRowKind, AddItem[]>();
    for (const item of filtered) {
      const bucket = byKind.get(item.kind) ?? [];
      bucket.push(item);
      byKind.set(item.kind, bucket);
    }
    return ADD_GROUP_ORDER.map((kind) => ({
      kind,
      label: addGroupLabel(kind),
      items: byKind.get(kind) ?? [],
    })).filter((g) => g.items.length > 0);
  }, [filtered]);

  const onAdd = (item: AddItem) => {
    if (item.payload.source === "hidden") {
      onShowExisting(item.payload.entry);
    } else {
      flipSidebar.mutate({ id: item.payload.viewId, show: true });
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Add to sidebar</DialogTitle>
          <DialogDescription>
            Pick something to bring into the sidebar. Removed defaults appear
            here too — adding restores them.
          </DialogDescription>
        </DialogHeader>
        <div className="relative">
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 h-4 w-4 -translate-y-1/2" />
          <Input
            type="search"
            placeholder="Search items…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="pl-8"
            aria-label="Filter items"
          />
        </div>
        <div className="max-h-[50vh] space-y-2 overflow-y-auto py-1">
          {allItems.length === 0 ? (
            <p className="text-muted-foreground py-4 text-sm">
              Nothing left to add. Create more saved views or pages first.
            </p>
          ) : groups.length === 0 ? (
            <p className="text-muted-foreground py-4 text-sm">
              Nothing matches <span className="text-foreground">{query}</span>.
            </p>
          ) : (
            groups.map((group) => (
              <div key={group.kind} className="space-y-1">
                <p className="bg-background text-muted-foreground/70 sticky top-0 z-10 px-2 pt-1 pb-0.5 text-[10px] font-medium tracking-widest uppercase">
                  {group.label}
                </p>
                {group.items.map((item) => (
                  <AddRow
                    key={item.id}
                    label={item.label}
                    description={item.description ?? null}
                    kind={item.kind}
                    onAdd={() => onAdd(item)}
                  />
                ))}
              </div>
            ))
          )}
        </div>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function AddRow({
  label,
  description,
  kind,
  onAdd,
}: {
  label: string;
  description?: string | null;
  kind: AddRowKind;
  onAdd: () => void;
}) {
  return (
    <div className="hover:bg-secondary/50 flex items-center gap-3 rounded-md px-2 py-2">
      <div className="min-w-0 flex-1">
        <span className="block truncate text-sm">{label}</span>
        {description ? (
          <span className="text-muted-foreground block truncate text-xs">
            {description}
          </span>
        ) : null}
      </div>
      <Badge
        variant="outline"
        className="text-muted-foreground hidden shrink-0 text-[10px] font-medium uppercase tracking-wider sm:inline-flex"
      >
        {addRowKindLabel(kind)}
      </Badge>
      <Button type="button" variant="outline" size="sm" onClick={onAdd}>
        <Plus className="mr-1 h-3.5 w-3.5" />
        Add
      </Button>
    </div>
  );
}

// Re-exported here so co-located callers can stick to one import path.
// (The actual file does the heavy lifting.)
export type { SidebarLayoutView };
