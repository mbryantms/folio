"use client";

/**
 * Grouped board for one tab of Settings → Views.
 *
 * Renders the user's groups (named, reorderable, collapsible buckets)
 * followed by the implicit "Ungrouped" section, with every saved view
 * rendered through the caller's `renderCard`. Drag a card anywhere —
 * into another group, back to Ungrouped, or to a new position inside its
 * section (Ungrouped included) — and the whole scope's layout is
 * committed in one request the moment it drops
 * (`useReplaceViewGroupLayout`, optimistic). Groups reorder by their grip
 * handle. Everything also works without a
 * pointer: the card's ⋯ menu has "Move to group", group menus have Move
 * up / down, and the grip handles are keyboard-sortable.
 *
 * Layout logic is pure and lives in `@/lib/saved-views/view-board`;
 * this file is the dnd-kit + UI shell around it.
 */

import * as React from "react";
import {
  DndContext,
  DragOverlay,
  defaultDropAnimationSideEffects,
  type DropAnimation,
  KeyboardSensor,
  MouseSensor,
  TouchSensor,
  closestCenter,
  pointerWithin,
  rectIntersection,
  useSensor,
  useSensors,
  type CollisionDetection,
  type DragEndEvent,
  type DragOverEvent,
  type DragStartEvent,
  type UniqueIdentifier,
} from "@dnd-kit/core";
import {
  SortableContext,
  rectSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { useDroppable } from "@dnd-kit/core";
import { CSS } from "@dnd-kit/utilities";
import {
  ArrowDown,
  ArrowUp,
  ChevronRight,
  FolderPlus,
  GripVertical,
  MoreHorizontal,
  Pencil,
  Search,
  Trash2,
  X,
  type LucideIcon,
} from "lucide-react";

import { CollectionCardSkeleton } from "@/components/collections/CollectionCardSkeleton";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/folio/select";
import {
  BoardToolbarControlsSkeleton,
  ViewCardsSkeleton,
} from "@/components/skeletons/SettingsShapes";
import { useQueryClient } from "@tanstack/react-query";

import {
  useCreateViewGroup,
  useReplaceViewGroupLayout,
  useUpdateViewGroup,
} from "@/lib/api/mutations";
import { apiMutate, useApiMutation } from "@/lib/api/mutations/_core";
import { jsonFetch, queryKeys, useViewGroups } from "@/lib/api/queries";
import type {
  ReplaceLayoutReq,
  SavedViewView,
  ViewGroupListView,
  ViewGroupScope,
  ViewGroupView,
} from "@/lib/api/types";
import {
  UNGROUPED_ID,
  VIEW_SORT_OPTIONS,
  boardToLayout,
  buildViewBoard,
  containerOf,
  countViews,
  filterBoard,
  moveGroup,
  moveView,
  type BoardGroup,
  type ViewBoard,
  type ViewSort,
} from "@/lib/saved-views/view-board";
import { cn } from "@/lib/utils";
import {
  DRAG_LIFT_CLASS,
  LIFT_TRANSITION,
  SPRING_EASING,
  useSpringSort,
} from "@/lib/ui/springy-drag";
import { useReducedMotion } from "@/lib/visual-effects";
import { showUndoToast } from "@/lib/ui/undo-toast";

// ──────────────────── delete + undo ────────────────────

/** The layout PUT that puts a re-created group back where the deleted
 *  one sat. `before` is the scope's layout captured just before the
 *  delete; `current` is the server's list after the group was re-created
 *  under `newId`. Group order follows `before` (old id swapped for the
 *  new one); groups made since trail it. Members come from `current`, so
 *  moves made in the meantime aren't clobbered, and the ungrouped order
 *  is the pre-delete one minus anything that has joined a group since.
 *  Exported for tests. */
export function restoredGroupLayout(
  scope: ViewGroupScope,
  before: ReturnType<typeof boardToLayout>,
  oldId: string,
  newId: string,
  current: ViewGroupListView,
): ReplaceLayoutReq {
  const byId = new Map(current.groups.map((g) => [g.id, g]));
  const order = before.groups.map((g) => (g.id === oldId ? newId : g.id));
  for (const g of current.groups) if (!order.includes(g.id)) order.push(g.id);
  const groups = order.flatMap((id) => {
    const g = byId.get(id);
    return g ? [{ id, view_ids: g.view_ids }] : [];
  });
  const grouped = new Set(groups.flatMap((g) => g.view_ids));
  return {
    scope,
    groups,
    ungrouped: before.ungrouped.filter((id) => !grouped.has(id)),
  };
}

/** Delete a group immediately (optimistically dropped from the cache) and
 *  offer Undo, which re-creates it with the same name, members, collapsed
 *  state and slot. Only the grouping is ever lost — the views themselves
 *  just fall back to Ungrouped — so no confirm is needed. */
function useDeleteGroupWithUndo(scope: ViewGroupScope) {
  const qc = useQueryClient();
  const key = queryKeys.viewGroups(scope);
  const del = useApiMutation<unknown, string>(
    (id) => ({ path: `/me/view-groups/${id}`, method: "DELETE" }),
    {
      onMutate: async (id) => {
        await qc.cancelQueries({ queryKey: key });
        const snapshot = qc.getQueryData<ViewGroupListView>(key);
        if (snapshot) {
          qc.setQueryData<ViewGroupListView>(key, {
            ...snapshot,
            groups: snapshot.groups.filter((g) => g.id !== id),
          });
        }
        return { snapshot };
      },
      onError: (_err, _id, ctx) => {
        const c = ctx as { snapshot?: ViewGroupListView } | undefined;
        if (c?.snapshot) qc.setQueryData(key, c.snapshot);
      },
      onSettled: () => {
        qc.invalidateQueries({ queryKey: key });
      },
    },
  );
  const mutate = del.mutate;

  return React.useCallback(
    (group: BoardGroup, board: ViewBoard) => {
      const before = boardToLayout(board);
      const viewIds = group.views.map((v) => v.id);
      mutate(group.id, {
        onSuccess: () =>
          showUndoToast({
            message: `Deleted group "${group.name}"`,
            description:
              viewIds.length > 0 ? "Its views moved to Ungrouped." : undefined,
            restoredMessage: `Restored group "${group.name}"`,
            failedMessage: "Couldn't restore the group",
            onUndo: async () => {
              try {
                const created = await apiMutate<ViewGroupView>({
                  path: "/me/view-groups",
                  method: "POST",
                  body: { scope, name: group.name, view_ids: viewIds },
                });
                if (!created) throw new Error("create returned null");
                if (group.collapsed) {
                  await apiMutate({
                    path: `/me/view-groups/${created.id}`,
                    method: "PATCH",
                    body: { collapsed: true },
                  });
                }
                const current = await jsonFetch<ViewGroupListView>(
                  `/me/view-groups?scope=${encodeURIComponent(scope)}`,
                );
                await apiMutate({
                  path: "/me/view-groups/layout",
                  method: "PUT",
                  body: restoredGroupLayout(
                    scope,
                    before,
                    group.id,
                    created.id,
                    current,
                  ),
                });
              } finally {
                qc.invalidateQueries({
                  queryKey: queryKeys.viewGroups(scope),
                });
              }
            },
          }),
      });
    },
    [mutate, qc, scope],
  );
}

// ──────────────────── ids ────────────────────

const GROUP_PREFIX = "group:";
const VIEW_PREFIX = "view:";
const groupDndId = (id: string) => `${GROUP_PREFIX}${id}`;
const viewDndId = (id: string) => `${VIEW_PREFIX}${id}`;

function parseDndId(
  id: UniqueIdentifier,
): { type: "group" | "view" | "ungrouped"; id: string } | null {
  const s = String(id);
  if (s === UNGROUPED_ID) return { type: "ungrouped", id: UNGROUPED_ID };
  if (s.startsWith(GROUP_PREFIX))
    return { type: "group", id: s.slice(GROUP_PREFIX.length) };
  if (s.startsWith(VIEW_PREFIX))
    return { type: "view", id: s.slice(VIEW_PREFIX.length) };
  return null;
}

// ──────────────────── card-side actions context ────────────────────

export type ViewBoardActions = {
  groups: ReadonlyArray<{ id: string; name: string }>;
  /** Current container of a view (`UNGROUPED_ID` when none). */
  containerOf: (viewId: string) => string | null;
  /** Move a view into a group (`null` = ungroup). Commits immediately. */
  moveToGroup: (viewId: string, groupId: string | null) => void;
  /** Open the New-group dialog with this view pre-added. */
  newGroupWithView: (viewId: string) => void;
};

const ViewBoardActionsContext = React.createContext<ViewBoardActions | null>(
  null,
);

/** `null` when the card renders outside a board (e.g. a plain list). */
export function useViewBoardActions(): ViewBoardActions | null {
  return React.useContext(ViewBoardActionsContext);
}

// ──────────────────── board ────────────────────

export type GroupedViewsBoardProps = {
  scope: ViewGroupScope;
  views: SavedViewView[];
  loading: boolean;
  /** Shown when there are no views at all (groups are irrelevant then). */
  empty: React.ReactNode;
  /** Short line under the tab header explaining what this kind is. */
  blurb: string;
  /** Primary action (New filter view / Import CBL / New collection). */
  action: React.ReactNode;
  /** Noun for copy: "list", "view", "collection". */
  noun: string;
  icon: LucideIcon;
  renderCard: (view: SavedViewView) => React.ReactNode;
  /** Extra items appended to each group's ⋯ menu (e.g. "Update lists in
   *  this group" on the reading-lists tab). */
  groupMenuExtra?: (group: BoardGroup) => React.ReactNode;
  /** Optional panel rendered between the toolbar and the board (the
   *  reading-lists tab mounts its updates panel here). */
  panel?: React.ReactNode;
  /** Card grid density: `compact` (Settings → Views, many small cards)
   *  or `large` (the `/collections` page: fixed 15rem cards, big covers). */
  size?: "compact" | "large";
};

const CARD_GRID_CLASS = {
  compact:
    "grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5 xl:grid-cols-6",
  // Fixed-width cards, so a wide screen adds cards per row instead of
  // blowing each one up; phones keep two flexible columns.
  large:
    "grid grid-cols-2 gap-3 sm:grid-cols-[repeat(auto-fill,15rem)] sm:gap-4",
} as const;
const CardGridClassContext = React.createContext<string>(
  CARD_GRID_CLASS.compact,
);

/** The held card springs into its slot on drop and settles flat as it
 *  lands (its lift class comes off, and `LIFT_TRANSITION` springs the
 *  scale / tilt / shadow back), like the Want to Read grid's in-place
 *  drag. The slot (the real card, hidden while held) shows once it's
 *  down. */
const DROP_ANIMATION: DropAnimation = {
  duration: 280,
  easing: SPRING_EASING,
  sideEffects: (args) => {
    args.dragOverlay.node
      .querySelector(`.${DRAG_LIFT_CLASS}`)
      ?.classList.remove(DRAG_LIFT_CLASS);
    return defaultDropAnimationSideEffects({
      styles: { active: { opacity: "0" } },
    })(args);
  },
};

export function GroupedViewsBoard({
  scope,
  views,
  loading,
  empty,
  blurb,
  action,
  noun,
  icon: Icon,
  renderCard,
  groupMenuExtra,
  panel,
  size = "compact",
}: GroupedViewsBoardProps) {
  const groupsQ = useViewGroups(scope);
  const replaceLayout = useReplaceViewGroupLayout(scope);
  const createGroup = useCreateViewGroup();
  const updateGroup = useUpdateViewGroup(scope);
  const deleteGroup = useDeleteGroupWithUndo(scope);

  const [sort, setSort] = React.useState<ViewSort>("custom");
  const [query, setQuery] = React.useState("");
  const serverBoard = React.useMemo(
    () =>
      buildViewBoard(
        views,
        groupsQ.data?.groups ?? [],
        groupsQ.data?.ungrouped_ids ?? [],
        sort,
      ),
    [views, groupsQ.data, sort],
  );
  // While a drag is in flight (and until the commit settles) the board
  // renders from `draft`; afterwards it falls back to the query cache,
  // which the layout mutation already patched optimistically.
  const [draft, setDraft] = React.useState<ViewBoard | null>(null);
  const board = draft ?? serverBoard;
  const visibleBoard = React.useMemo(
    () => filterBoard(board, query),
    [board, query],
  );
  const searching = query.trim().length > 0;

  const [activeId, setActiveId] = React.useState<UniqueIdentifier | null>(null);
  const reduced = useReducedMotion();
  // Where the held card started, so the section it is moving into (and
  // only that one) can light up as the drop target.
  const [dragFrom, setDragFrom] = React.useState<string | null>(null);
  const [newGroupOpen, setNewGroupOpen] = React.useState(false);
  const [newGroupSeedView, setNewGroupSeedView] = React.useState<string | null>(
    null,
  );
  // Clicks that land right after a drop would navigate the card's link;
  // swallow them for a beat (see `BoardCard`).
  const suppressClickUntil = React.useRef(0);

  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: { distance: 6 } }),
    useSensor(TouchSensor, {
      activationConstraint: { delay: 220, tolerance: 8 },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const commit = React.useCallback(
    (next: ViewBoard) => {
      setDraft(next);
      // The PUT pins the ungrouped order exactly as displayed. If that
      // order differs from the server's while a non-custom sort is on,
      // switch to Custom so the pinned order stays what the user sees
      // instead of re-sorting under them once the draft clears.
      if (
        sort !== "custom" &&
        !sameIds(next.ungrouped, serverBoard.ungrouped)
      ) {
        setSort("custom");
      }
      replaceLayout.mutate(
        { scope, ...boardToLayout(next) },
        { onSettled: () => setDraft(null) },
      );
    },
    [replaceLayout, scope, serverBoard, sort],
  );

  // ── actions shared with cards (menu fallback for DnD) ──
  const actions = React.useMemo<ViewBoardActions>(
    () => ({
      groups: board.groups.map((g) => ({ id: g.id, name: g.name })),
      containerOf: (viewId) => containerOf(board, viewId),
      moveToGroup: (viewId, groupId) => {
        const next = moveView(board, viewId, groupId ?? UNGROUPED_ID);
        if (next !== board) commit(next);
      },
      newGroupWithView: (viewId) => {
        setNewGroupSeedView(viewId);
        setNewGroupOpen(true);
      },
    }),
    [board, commit],
  );

  // ── dnd handlers ──
  const resolveContainer = React.useCallback(
    (over: UniqueIdentifier): string | null => {
      const parsed = parseDndId(over);
      if (!parsed) return null;
      if (parsed.type === "view") return containerOf(board, parsed.id);
      return parsed.id;
    },
    [board],
  );

  const collisionDetection = React.useCallback<CollisionDetection>((args) => {
    const active = parseDndId(args.active.id);
    if (active?.type === "group") {
      return closestCenter({
        ...args,
        droppableContainers: args.droppableContainers.filter(
          (c) => parseDndId(c.id)?.type === "group",
        ),
      });
    }
    const within = pointerWithin(args);
    const collisions = within.length > 0 ? within : rectIntersection(args);
    // A card and its container both collide; the card wins so the drop
    // index comes from a neighbour, not the whole section.
    const card = collisions.find((c) => parseDndId(c.id)?.type === "view");
    return card ? [card] : collisions.slice(0, 1);
  }, []);

  function onDragStart(ev: DragStartEvent) {
    const a = parseDndId(ev.active.id);
    setDragFrom(a?.type === "view" ? containerOf(board, a.id) : null);
    setActiveId(ev.active.id);
    setDraft(board);
  }

  function onDragOver(ev: DragOverEvent) {
    const { active, over } = ev;
    const a = parseDndId(active.id);
    if (!over || a?.type !== "view") return;
    const overContainer = resolveContainer(over.id);
    const fromContainer = containerOf(board, a.id);
    if (!overContainer || overContainer === fromContainer) return;
    // Cross-container hover: tentatively move the card so the target
    // section opens a slot where the pointer is.
    const overParsed = parseDndId(over.id);
    let index: number | undefined;
    if (overParsed?.type === "view") {
      const target = viewsIn(board, overContainer);
      const overIndex = target.findIndex((v) => v.id === overParsed.id);
      if (overIndex >= 0) {
        const translated = active.rect.current.translated;
        const below =
          translated != null &&
          translated.top > over.rect.top + over.rect.height / 2;
        index = overIndex + (below ? 1 : 0);
      }
    }
    setDraft(moveView(board, a.id, overContainer, index));
  }

  function finishDrag() {
    setActiveId(null);
    suppressClickUntil.current = Date.now() + 200;
  }

  function onDragEnd(ev: DragEndEvent) {
    const { active, over } = ev;
    const a = parseDndId(active.id);
    finishDrag();
    if (!over || !a) {
      setDraft(null);
      return;
    }
    if (a.type === "group") {
      const o = parseDndId(over.id);
      const from = board.groups.findIndex((g) => g.id === a.id);
      const to = board.groups.findIndex((g) => g.id === o?.id);
      const next = moveGroup(board, from, to);
      if (next === board) setDraft(null);
      else commit(next);
      return;
    }
    if (a.type === "view") {
      const overContainer = resolveContainer(over.id);
      const fromContainer = containerOf(board, a.id);
      let next = board;
      if (overContainer && overContainer === fromContainer) {
        // Same-section reorder (a group or Ungrouped).
        const list = viewsIn(board, overContainer);
        const o = parseDndId(over.id);
        const from = list.findIndex((v) => v.id === a.id);
        const to =
          o?.type === "view" ? list.findIndex((v) => v.id === o.id) : -1;
        if (from >= 0 && to >= 0 && from !== to) {
          next = moveView(board, a.id, overContainer, to);
        }
      } else if (overContainer && overContainer !== fromContainer) {
        next = moveView(board, a.id, overContainer);
      }
      // Compare against what the server has; the draft may already hold
      // the move from onDragOver.
      const changed =
        JSON.stringify(boardToLayout(next)) !==
        JSON.stringify(boardToLayout(serverBoard));
      if (changed) commit(next);
      else setDraft(null);
    }
  }

  function onDragCancel() {
    finishDrag();
    setDraft(null);
  }

  // ── group lifecycle ──
  function submitNewGroup(name: string) {
    createGroup.mutate(
      {
        scope,
        name,
        view_ids: newGroupSeedView ? [newGroupSeedView] : [],
      },
      {
        onSuccess: () => {
          setNewGroupOpen(false);
          setNewGroupSeedView(null);
        },
      },
    );
  }

  const activeParsed = activeId ? parseDndId(activeId) : null;
  // The section a held card will land in when it's leaving its own.
  const heldIn =
    activeParsed?.type === "view" ? containerOf(board, activeParsed.id) : null;
  const dropTarget = heldIn && heldIn !== dragFrom ? heldIn : null;
  const activeView =
    activeParsed?.type === "view"
      ? (views.find((v) => v.id === activeParsed.id) ?? null)
      : null;
  const activeGroup =
    activeParsed?.type === "group"
      ? (board.groups.find((g) => g.id === activeParsed.id) ?? null)
      : null;

  const total = countViews(board);
  const showGroups = board.groups.length > 0;

  return (
    <ViewBoardActionsContext.Provider value={actions}>
      <CardGridClassContext.Provider value={CARD_GRID_CLASS[size]}>
        <div className="space-y-4">
          {/* Toolbar: blurb + search + sort + new group + primary action */}
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-muted-foreground text-sm">{blurb}</p>
            <div className="flex flex-wrap items-center gap-2 max-sm:w-full">
              {total > 0 ? (
                <>
                  <div className="relative max-sm:basis-full">
                    <Search
                      aria-hidden="true"
                      className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
                    />
                    <Input
                      type="search"
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      placeholder={`Search ${noun}s…`}
                      aria-label={`Search ${noun}s`}
                      className="h-9 w-full pl-8 sm:w-56"
                    />
                  </div>
                  <Select
                    value={sort}
                    onValueChange={(v) => setSort(v as ViewSort)}
                  >
                    <SelectTrigger
                      className="h-9 w-[11.5rem]"
                      aria-label="Sort ungrouped"
                      title="Order of the ungrouped section. Dragging a card pins the custom order."
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent align="end">
                      {VIEW_SORT_OPTIONS.map((o) => (
                        <SelectItem key={o.value} value={o.value}>
                          {o.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    type="button"
                    variant="outline"
                    className="h-9"
                    onClick={() => {
                      setNewGroupSeedView(null);
                      setNewGroupOpen(true);
                    }}
                  >
                    <FolderPlus className="mr-1.5 size-4" /> New group
                  </Button>
                </>
              ) : loading || groupsQ.isLoading ? (
                // Hold the controls' room while the board loads, so the
                // primary action doesn't jump left once cards arrive.
                <div aria-hidden className="contents">
                  <BoardToolbarControlsSkeleton />
                </div>
              ) : null}
              {action}
            </div>
          </div>

          {panel}

          {loading || groupsQ.isLoading ? (
            <BoardSkeleton />
          ) : total === 0 ? (
            <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-6 text-sm">
              {empty}
            </div>
          ) : (
            <DndContext
              sensors={sensors}
              collisionDetection={collisionDetection}
              onDragStart={onDragStart}
              onDragOver={onDragOver}
              onDragEnd={onDragEnd}
              onDragCancel={onDragCancel}
            >
              {/* While dragging, cards under the pointer must not react
                (hover wash, cover fan, tint): the slot and the target
                group's ring are the only feedback. Collision detection
                uses rects, not pointer events. */}
              <div
                className={cn(
                  "space-y-4",
                  activeId &&
                    "select-none [&_[data-board-card]]:pointer-events-none",
                )}
              >
                <SortableContext
                  items={board.groups.map((g) => groupDndId(g.id))}
                  strategy={verticalListSortingStrategy}
                >
                  {visibleBoard.groups.map((group, idx) => (
                    <GroupSection
                      key={group.id}
                      group={group}
                      totalInGroup={board.groups[idx]?.views.length ?? 0}
                      forceOpen={searching}
                      noun={noun}
                      isFirst={idx === 0}
                      isLast={idx === board.groups.length - 1}
                      dragActive={activeId !== null}
                      isDropTarget={dropTarget === group.id}
                      suppressClickUntil={suppressClickUntil}
                      renderCard={renderCard}
                      onRename={(name) =>
                        updateGroup.mutate({ id: group.id, name })
                      }
                      onToggleCollapsed={() =>
                        updateGroup.mutate({
                          id: group.id,
                          collapsed: !group.collapsed,
                        })
                      }
                      onMove={(dir) => {
                        const next = moveGroup(board, idx, idx + dir);
                        if (next !== board) commit(next);
                      }}
                      onDelete={() =>
                        deleteGroup(
                          board.groups.find((g) => g.id === group.id) ?? group,
                          board,
                        )
                      }
                      menuExtra={groupMenuExtra?.(board.groups[idx] ?? group)}
                    />
                  ))}
                </SortableContext>

                <UngroupedSection
                  views={visibleBoard.ungrouped}
                  total={board.ungrouped.length}
                  hasGroups={showGroups}
                  noun={noun}
                  icon={Icon}
                  searching={searching}
                  dragActive={activeId !== null}
                  isDropTarget={dropTarget === UNGROUPED_ID}
                  suppressClickUntil={suppressClickUntil}
                  renderCard={renderCard}
                />
              </div>

              <DragOverlay dropAnimation={reduced ? null : DROP_ANIMATION}>
                {activeView ? (
                  // A lifted copy of the card itself, at its own size, so
                  // what you hold is what lands.
                  <div
                    className={cn(
                      "pointer-events-none rounded-lg",
                      DRAG_LIFT_CLASS,
                    )}
                    style={{ transition: LIFT_TRANSITION }}
                  >
                    {renderCard(activeView)}
                  </div>
                ) : activeGroup ? (
                  <DragGhost
                    icon={GripVertical}
                    label={activeGroup.name}
                    meta={`${activeGroup.views.length} ${plural(noun, activeGroup.views.length)}`}
                  />
                ) : null}
              </DragOverlay>
            </DndContext>
          )}
        </div>

        <NewGroupDialog
          open={newGroupOpen}
          onOpenChange={(o) => {
            setNewGroupOpen(o);
            if (!o) setNewGroupSeedView(null);
          }}
          seedViewName={
            newGroupSeedView
              ? views.find((v) => v.id === newGroupSeedView)?.name
              : undefined
          }
          pending={createGroup.isPending}
          onSubmit={submitNewGroup}
          noun={noun}
        />
      </CardGridClassContext.Provider>
    </ViewBoardActionsContext.Provider>
  );
}

function plural(noun: string, n: number): string {
  return n === 1 ? noun : `${noun}s`;
}

/** Views in a container (`UNGROUPED_ID` or a group id). */
function viewsIn(board: ViewBoard, container: string): SavedViewView[] {
  if (container === UNGROUPED_ID) return board.ungrouped;
  return board.groups.find((g) => g.id === container)?.views ?? [];
}

function sameIds(a: readonly SavedViewView[], b: readonly SavedViewView[]) {
  return a.length === b.length && a.every((v, i) => v.id === b[i]?.id);
}

// ──────────────────── group section ────────────────────

function GroupSection({
  group,
  totalInGroup,
  forceOpen,
  noun,
  isFirst,
  isLast,
  dragActive,
  isDropTarget,
  suppressClickUntil,
  renderCard,
  onRename,
  onToggleCollapsed,
  onMove,
  onDelete,
  menuExtra,
}: {
  group: BoardGroup;
  /** Unfiltered member count (the badge), vs `group.views` which may be
   *  narrowed by search. */
  totalInGroup: number;
  forceOpen: boolean;
  noun: string;
  isFirst: boolean;
  isLast: boolean;
  dragActive: boolean;
  /** A held card from another section will land here. */
  isDropTarget: boolean;
  suppressClickUntil: React.MutableRefObject<number>;
  renderCard: (view: SavedViewView) => React.ReactNode;
  onRename: (name: string) => void;
  onToggleCollapsed: () => void;
  onMove: (dir: -1 | 1) => void;
  onDelete: () => void;
  menuExtra?: React.ReactNode;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: groupDndId(group.id),
    data: { type: "group" },
    transition: useSpringSort(),
  });
  const [editing, setEditing] = React.useState(false);
  const open = forceOpen || !group.collapsed;
  const panelId = React.useId();
  // Highlight when a *card* hovers this section (not another group).
  return (
    <section
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition }}
      aria-label={`Group ${group.name}`}
      className={cn(
        "border-border/60 bg-card/40 rounded-xl border transition-shadow",
        isDragging && "opacity-50",
        isDropTarget && "ring-primary/60 bg-primary/5 ring-2",
      )}
    >
      <header className="flex items-center gap-1.5 px-2 py-1.5 sm:px-3">
        <button
          type="button"
          ref={setActivatorNodeRef}
          {...attributes}
          {...listeners}
          aria-label={`Drag to reorder group ${group.name}`}
          className="text-muted-foreground hover:text-foreground focus-visible:ring-ring -ml-1 inline-flex h-8 w-6 shrink-0 cursor-grab touch-none items-center justify-center rounded focus-visible:ring-2 focus-visible:outline-none active:cursor-grabbing"
        >
          <GripVertical className="size-4" aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onToggleCollapsed}
          aria-expanded={open}
          aria-controls={panelId}
          disabled={forceOpen}
          aria-label={open ? `Collapse ${group.name}` : `Expand ${group.name}`}
          className="text-muted-foreground hover:text-foreground focus-visible:ring-ring inline-flex h-8 w-7 shrink-0 items-center justify-center rounded focus-visible:ring-2 focus-visible:outline-none disabled:opacity-50"
        >
          <ChevronRight
            aria-hidden="true"
            className={cn("size-4 transition-transform", open && "rotate-90")}
          />
        </button>

        {editing ? (
          <GroupNameEditor
            initial={group.name}
            onCancel={() => setEditing(false)}
            onCommit={(name) => {
              setEditing(false);
              if (name && name !== group.name) onRename(name);
            }}
          />
        ) : (
          <button
            type="button"
            onClick={onToggleCollapsed}
            disabled={forceOpen}
            className="focus-visible:ring-ring min-w-0 flex-1 truncate rounded text-left text-sm font-semibold focus-visible:ring-2 focus-visible:outline-none disabled:cursor-default"
            title={group.name}
          >
            {group.name}
          </button>
        )}
        <Badge
          variant="secondary"
          className="h-5 min-w-5 shrink-0 justify-center px-1.5 tabular-nums"
          aria-label={`${totalInGroup} ${plural(noun, totalInGroup)}`}
        >
          {totalInGroup}
        </Badge>
        {!editing ? (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={`Rename ${group.name}`}
            className="text-muted-foreground hover:text-foreground size-8 shrink-0"
            onClick={() => setEditing(true)}
          >
            <Pencil className="size-3.5" />
          </Button>
        ) : null}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={`Actions for group ${group.name}`}
              className="text-muted-foreground hover:text-foreground size-8 shrink-0"
            >
              <MoreHorizontal className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-56">
            <DropdownMenuItem onSelect={() => setEditing(true)}>
              <Pencil className="size-4" /> Rename
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onMove(-1)} disabled={isFirst}>
              <ArrowUp className="size-4" /> Move up
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onMove(1)} disabled={isLast}>
              <ArrowDown className="size-4" /> Move down
            </DropdownMenuItem>
            {menuExtra ? (
              <>
                <DropdownMenuSeparator />
                {menuExtra}
              </>
            ) : null}
            <DropdownMenuSeparator />
            <DropdownMenuItem
              onSelect={onDelete}
              className="text-destructive focus:text-destructive"
            >
              <Trash2 className="size-4" /> Delete group
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </header>

      {/* The sortable context stays mounted while collapsed so a card
          dropped on the header still has a target; only the grid hides. */}
      <SortableContext
        items={group.views.map((v) => viewDndId(v.id))}
        strategy={rectSortingStrategy}
      >
        <div id={panelId} hidden={!open} className="px-2 pb-2 sm:px-3 sm:pb-3">
          {group.views.length === 0 ? (
            <div
              className={cn(
                "border-border/60 text-muted-foreground flex min-h-20 items-center justify-center rounded-lg border border-dashed text-sm transition-colors",
                dragActive && "border-primary/40 text-foreground/80",
              )}
            >
              {dragActive
                ? `Drop ${plural(noun, 2)} here`
                : `Empty group — drag ${plural(noun, 2)} here or use a card's menu.`}
            </div>
          ) : (
            <CardGrid
              views={group.views}
              container={group.id}
              suppressClickUntil={suppressClickUntil}
              renderCard={renderCard}
            />
          )}
        </div>
      </SortableContext>
    </section>
  );
}

function GroupNameEditor({
  initial,
  onCommit,
  onCancel,
}: {
  initial: string;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = React.useState(initial);
  const committed = React.useRef(false);
  function commit() {
    if (committed.current) return;
    committed.current = true;
    onCommit(value.trim());
  }
  return (
    <div className="flex min-w-0 flex-1 items-center gap-1">
      <Input
        autoFocus
        value={value}
        maxLength={80}
        aria-label="Group name"
        onChange={(e) => setValue(e.target.value)}
        onFocus={(e) => e.currentTarget.select()}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            committed.current = true;
            onCancel();
          }
        }}
        className="h-8 text-sm font-semibold"
      />
      <Button
        type="button"
        variant="ghost"
        size="icon"
        aria-label="Cancel rename"
        className="size-8 shrink-0"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => {
          committed.current = true;
          onCancel();
        }}
      >
        <X className="size-4" />
      </Button>
    </div>
  );
}

// ──────────────────── ungrouped section ────────────────────

function UngroupedSection({
  views,
  total,
  hasGroups,
  noun,
  icon: Icon,
  searching,
  dragActive,
  isDropTarget,
  suppressClickUntil,
  renderCard,
}: {
  views: SavedViewView[];
  total: number;
  hasGroups: boolean;
  noun: string;
  icon: LucideIcon;
  searching: boolean;
  dragActive: boolean;
  /** A held card from another section will land here. */
  isDropTarget: boolean;
  suppressClickUntil: React.MutableRefObject<number>;
  renderCard: (view: SavedViewView) => React.ReactNode;
}) {
  const { setNodeRef } = useDroppable({ id: UNGROUPED_ID });
  // Without any groups the "Ungrouped" framing is noise — render a plain
  // grid. Once a group exists the remainder needs a labelled home.
  const framed = hasGroups;

  return (
    <section
      ref={setNodeRef}
      aria-label={framed ? "Ungrouped" : undefined}
      className={cn(
        framed &&
          "rounded-xl border border-dashed border-transparent p-2 transition-colors sm:p-3",
        framed && isDropTarget && "border-primary/60 bg-primary/5",
        framed && dragActive && !isDropTarget && "border-border/60",
      )}
    >
      {framed ? (
        <header className="mb-2 flex items-center gap-2 px-1">
          <Icon className="text-muted-foreground size-4" aria-hidden="true" />
          <h3 className="text-muted-foreground text-sm font-semibold">
            Ungrouped
          </h3>
          <Badge
            variant="outline"
            className="h-5 min-w-5 justify-center px-1.5 tabular-nums"
          >
            {total}
          </Badge>
        </header>
      ) : null}
      <SortableContext
        items={views.map((v) => viewDndId(v.id))}
        strategy={rectSortingStrategy}
      >
        {views.length === 0 ? (
          <div
            className={cn(
              "text-muted-foreground flex min-h-16 items-center justify-center rounded-lg text-sm",
              framed && "border-border/60 border border-dashed",
            )}
          >
            {searching
              ? `No ${plural(noun, 2)} match your search.`
              : dragActive
                ? `Drop here to ungroup`
                : `Every ${noun} is in a group.`}
          </div>
        ) : (
          <CardGrid
            views={views}
            container={UNGROUPED_ID}
            suppressClickUntil={suppressClickUntil}
            renderCard={renderCard}
          />
        )}
      </SortableContext>
    </section>
  );
}

// ──────────────────── cards ────────────────────

function CardGrid({
  views,
  container,
  suppressClickUntil,
  renderCard,
}: {
  views: SavedViewView[];
  container: string;
  suppressClickUntil: React.MutableRefObject<number>;
  renderCard: (view: SavedViewView) => React.ReactNode;
}) {
  return (
    <ul role="list" className={React.useContext(CardGridClassContext)}>
      {views.map((view) => (
        <BoardCard
          key={view.id}
          view={view}
          container={container}
          suppressClickUntil={suppressClickUntil}
        >
          {renderCard(view)}
        </BoardCard>
      ))}
    </ul>
  );
}

/** Sortable wrapper around a card. Pointer drags start anywhere on the
 *  card except its buttons (so menus still open with a plain click);
 *  keyboard sorting goes through the grip handle, which is the element
 *  that carries the sortable ARIA attributes. */
function BoardCard({
  view,
  container,
  suppressClickUntil,
  children,
}: {
  view: SavedViewView;
  container: string;
  suppressClickUntil: React.MutableRefObject<number>;
  children: React.ReactNode;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: viewDndId(view.id),
    data: { type: "view", container },
    transition: useSpringSort(),
  });
  const { onKeyDown, ...pointerListeners } = (listeners ?? {}) as Record<
    string,
    (e: React.SyntheticEvent) => void
  >;

  return (
    <li
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition }}
      data-board-card=""
      className={cn(
        "group/card relative rounded-lg",
        // The card's slot while it's held: an empty gap the other cards
        // spring around (no outline), as on the Want to Read grid.
        isDragging && "[&>*]:invisible",
      )}
    >
      {/* The whole card is a pointer drag surface (the grip button is the
          keyboard / AT path, `attributes` + `onKeyDown` below). The
          listeners sit on a presentational wrapper so the list item keeps
          its semantics; `contents` keeps the grip's absolute positioning
          relative to the item. */}
      <div
        role="presentation"
        className="contents"
        // Native anchor/image drag would fight the pointer sensor.
        onDragStart={(e) => e.preventDefault()}
        onPointerDown={(e) => {
          // Buttons / menu items keep plain clicks; everything else starts
          // a drag after the sensor's distance threshold.
          const el = e.target as Element;
          if (el.closest("button, [role=menuitem], input, select, textarea"))
            return;
          pointerListeners.onPointerDown?.(e);
        }}
        onMouseDown={(e) => {
          const el = e.target as Element;
          if (el.closest("button, [role=menuitem], input, select, textarea"))
            return;
          pointerListeners.onMouseDown?.(e);
        }}
        onTouchStart={(e) => {
          const el = e.target as Element;
          if (el.closest("button, [role=menuitem], input, select, textarea"))
            return;
          pointerListeners.onTouchStart?.(e);
        }}
        onClickCapture={(e) => {
          if (Date.now() < suppressClickUntil.current) {
            e.preventDefault();
            e.stopPropagation();
          }
        }}
      >
        <button
          type="button"
          ref={setActivatorNodeRef}
          {...attributes}
          onKeyDown={onKeyDown}
          aria-label={`Drag ${view.name}`}
          className={cn(
            "text-muted-foreground hover:text-foreground bg-background/80 focus-visible:ring-ring absolute top-2 left-1 z-10 inline-flex h-7 w-5 cursor-grab touch-none items-center justify-center rounded opacity-0 backdrop-blur transition-opacity group-hover/card:opacity-100 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:outline-none active:cursor-grabbing",
            // Always visible on touch — there is no hover to reveal it.
            "[@media(pointer:coarse)]:opacity-100",
          )}
        >
          <GripVertical className="size-4" aria-hidden="true" />
        </button>
        {children}
      </div>
    </li>
  );
}

function DragGhost({
  icon: Icon,
  label,
  meta,
}: {
  icon: LucideIcon;
  label: string;
  meta?: string;
}) {
  return (
    <div
      className={cn(
        "bg-popover text-popover-foreground border-border flex max-w-xs items-center gap-2 rounded-lg border px-3 py-2 shadow-xl",
        DRAG_LIFT_CLASS,
      )}
    >
      <Icon
        className="text-muted-foreground size-4 shrink-0"
        aria-hidden="true"
      />
      <span className="truncate text-sm font-medium">{label}</span>
      {meta ? (
        <span className="text-muted-foreground shrink-0 text-xs">{meta}</span>
      ) : null}
    </div>
  );
}

function BoardSkeleton() {
  const grid = React.useContext(CardGridClassContext);
  const large = grid === CARD_GRID_CLASS.large;
  return (
    <div className="space-y-4" aria-busy="true" aria-label="Loading">
      {large ? (
        // Large (/collections): ungrouped collection cards, the same
        // card the route fallback draws.
        <ul className={grid}>
          {Array.from({ length: 4 }).map((_, i) => (
            <li key={i}>
              <CollectionCardSkeleton />
            </li>
          ))}
        </ul>
      ) : (
        // Compact (Settings → Views): ungrouped cards straight under the
        // toolbar, in the compact grid.
        <ViewCardsSkeleton />
      )}
    </div>
  );
}

// ──────────────────── new group dialog ────────────────────

function NewGroupDialog({
  open,
  onOpenChange,
  seedViewName,
  pending,
  onSubmit,
  noun,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  seedViewName?: string;
  pending: boolean;
  onSubmit: (name: string) => void;
  noun: string;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        {/* Radix unmounts the content on close, so the form's draft name
            resets for free — no effect needed. */}
        <NewGroupForm
          seedViewName={seedViewName}
          pending={pending}
          onSubmit={onSubmit}
          onCancel={() => onOpenChange(false)}
          noun={noun}
        />
      </DialogContent>
    </Dialog>
  );
}

function NewGroupForm({
  seedViewName,
  pending,
  onSubmit,
  onCancel,
  noun,
}: {
  seedViewName?: string;
  pending: boolean;
  onSubmit: (name: string) => void;
  onCancel: () => void;
  noun: string;
}) {
  const [name, setName] = React.useState("");
  const trimmed = name.trim();
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (trimmed) onSubmit(trimmed);
      }}
    >
      <DialogHeader>
        <DialogTitle>New group</DialogTitle>
        <DialogDescription>
          {seedViewName
            ? `“${seedViewName}” will be the first ${noun} in it. Drag more in later.`
            : `A named bucket for your ${plural(noun, 2)}. Drag ${plural(noun, 2)} in, or use a card's menu.`}
        </DialogDescription>
      </DialogHeader>
      <div className="py-4">
        <Input
          autoFocus
          value={name}
          maxLength={80}
          onChange={(e) => setName(e.target.value)}
          placeholder="e.g. Marvel events"
          aria-label="Group name"
        />
      </div>
      <DialogFooter>
        <Button
          type="button"
          variant="outline"
          onClick={onCancel}
          disabled={pending}
        >
          Cancel
        </Button>
        <Button type="submit" disabled={!trimmed || pending}>
          {pending ? "Creating…" : "Create group"}
        </Button>
      </DialogFooter>
    </form>
  );
}
