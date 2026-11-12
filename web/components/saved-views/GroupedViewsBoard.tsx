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
 * Two presentations share every bit of that behaviour: cards
 * (`renderCard`: filter views, collections) and rows (`rows`: the reading
 * lists' tree table, one column header with folder rows and list rows on
 * the `board-rows` grid, rendered from the `boardRows` row model). Only
 * the section and item shells differ (`GroupSection` / `BoardCard` vs
 * `GroupRowsSection` / `BoardRowItem`).
 *
 * Groups nest (folders inside folders, at most four levels). The rows
 * presentation offers it: a folder row drags into another folder (its
 * middle), before or after one (its top / bottom edge) or to the top
 * level, and its ⋯ menu has "New group inside" and "Move to…" for
 * keyboard and touch. Cards stay flat in what they offer; a nested group
 * there shows its whole path and keeps its place in the tree on save.
 * Deleting a group moves its contents up one level, and its Undo puts
 * the stored tree back.
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
  type Announcements,
  type CollisionDetection,
  type DragEndEvent,
  type DragMoveEvent,
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
import { useDraggable, useDroppable } from "@dnd-kit/core";
import { CSS } from "@dnd-kit/utilities";
import { createPortal } from "react-dom";
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderInput,
  FolderOpen,
  FolderPlus,
  GripVertical,
  MoreHorizontal,
  Pencil,
  Rows2,
  Rows4,
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
import { DropdownMenuResponsiveSub } from "@/components/folio/dropdown-menu-responsive-sub";
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
  ViewRowsSkeleton,
} from "@/components/skeletons/SettingsShapes";
import { LoadingRegion } from "@/components/skeletons/LoadingShapes";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  BOARD_CONTAINER,
  ROW_CELL,
  ROW_GRID,
  ROW_PAD,
} from "@/components/saved-views/board-rows";
import { useQueryClient } from "@tanstack/react-query";

import {
  useCreateViewGroup,
  useReplaceViewGroupLayout,
  useUpdateViewGroup,
} from "@/lib/api/mutations";
import { apiMutate, useApiMutation } from "@/lib/api/mutations/_core";
import { apiPath } from "@/lib/api/path";
import { jsonFetch, queryKeys, useViewGroups } from "@/lib/api/queries";
import type {
  SavedViewView,
  ViewGroupListView,
  ViewGroupScope,
  ViewGroupView,
} from "@/lib/api/types";
import {
  MAX_GROUP_DEPTH,
  UNGROUPED_ID,
  VIEW_SORT_OPTIONS,
  boardFromLayout,
  boardRows,
  boardToLayout,
  buildViewBoard,
  canNestInside,
  carryUnknownIds,
  childGroups,
  containerOf,
  countViews,
  filterBoard,
  findGroup,
  groupDepth,
  groupMoveRefusal,
  groupPath,
  layoutKeepingCustom,
  listWithoutGroup,
  moveGroup,
  moveGroupAmongSiblings,
  moveGroupTo,
  moveView,
  restoredGroupLayout,
  sortBoard,
  subtreeIds,
  subtreeViews,
  type BoardGroup,
  type BoardLayout,
  type BoardRow,
  type BuiltinViewSort,
  type GroupMoveRefusal,
  type ViewBoard,
  type ViewCompare,
  type ViewSort,
} from "@/lib/saved-views/view-board";
import {
  useBoardDensity,
  type BoardDensity,
} from "@/lib/saved-views/board-density";
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

export { restoredGroupLayout };

/** Delete a group immediately (optimistically, the way the server does
 *  it: its lists and child groups move up one level) and offer Undo,
 *  which re-creates it with the same name and collapsed state and puts
 *  the previous tree back: its slot among its siblings, its lists, and
 *  its child groups under it again. Only the grouping is ever lost — the
 *  views themselves just move up a level — so no confirm is needed. */
function useDeleteGroupWithUndo(scope: ViewGroupScope, noun: string) {
  const qc = useQueryClient();
  const key = queryKeys.viewGroups(scope);
  const del = useApiMutation<unknown, string>(
    (id) => ({ path: apiPath`/me/view-groups/${id}`, method: "DELETE" }),
    {
      onMutate: async (id) => {
        await qc.cancelQueries({ queryKey: key });
        const snapshot = qc.getQueryData<ViewGroupListView>(key);
        if (snapshot) qc.setQueryData(key, listWithoutGroup(snapshot, id));
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
    (group: BoardGroup, hasSubgroups: boolean) => {
      // The stored tree as the server has it (never a sorted display,
      // and with any id this page doesn't know), captured before the
      // optimistic delete rewrites the cache.
      const before = qc.getQueryData<ViewGroupListView>(key);
      const parts = [
        group.views.length > 0 ? plural(noun, 2) : null,
        hasSubgroups ? "groups" : null,
      ].filter((p): p is string => p !== null);
      const moved =
        parts.length > 0
          ? `Its ${parts.join(" and ")} moved up a level.`
          : undefined;
      mutate(group.id, {
        onSuccess: () =>
          showUndoToast({
            message: `Deleted group "${group.name}"`,
            description: moved,
            restoredMessage: `Restored group "${group.name}"`,
            failedMessage: "Couldn't restore the group",
            onUndo: async () => {
              try {
                if (!before) throw new Error("nothing to restore");
                // Created empty at the top level; the layout PUT below
                // puts it, its members and its children back.
                const created = await apiMutate<ViewGroupView>({
                  path: "/me/view-groups",
                  method: "POST",
                  body: { scope, name: group.name, view_ids: [] },
                });
                if (!created) throw new Error("create returned null");
                if (group.collapsed) {
                  await apiMutate({
                    path: apiPath`/me/view-groups/${created.id}`,
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
                qc.invalidateQueries({ queryKey: key });
              }
            },
          }),
      });
    },
    [mutate, qc, scope, key, noun],
  );
}

// ──────────────────── ids ────────────────────

const GROUP_PREFIX = "group:";
const HEAD_PREFIX = "head:";
const VIEW_PREFIX = "view:";
const groupDndId = (id: string) => `${GROUP_PREFIX}${id}`;
/** A folder row itself (rows presentation), apart from the section
 *  under it: where a held folder can land before, after or inside. */
const headDndId = (id: string) => `${HEAD_PREFIX}${id}`;
const viewDndId = (id: string) => `${VIEW_PREFIX}${id}`;

function parseDndId(
  id: UniqueIdentifier,
): { type: "group" | "head" | "view" | "ungrouped"; id: string } | null {
  const s = String(id);
  if (s === UNGROUPED_ID) return { type: "ungrouped", id: UNGROUPED_ID };
  if (s.startsWith(GROUP_PREFIX))
    return { type: "group", id: s.slice(GROUP_PREFIX.length) };
  if (s.startsWith(HEAD_PREFIX))
    return { type: "head", id: s.slice(HEAD_PREFIX.length) };
  if (s.startsWith(VIEW_PREFIX))
    return { type: "view", id: s.slice(VIEW_PREFIX.length) };
  return null;
}

/** Where a held folder will land (rows presentation). */
type FolderDrop = {
  parentId: string | null;
  /** Sibling index under `parentId` (end when undefined). */
  index: number | undefined;
  /** The folder row the line or ring shows on (`null`: the root). */
  target: string | null;
  zone: "before" | "after" | "into";
};

// ──────────────────── card-side actions context ────────────────────

export type ViewBoardActions = {
  /** Every group in tree order, with its level (1 = top) and the names
   *  from the top down to it. */
  groups: ReadonlyArray<{
    id: string;
    name: string;
    depth: number;
    path: string[];
  }>;
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
  /** Short line in the toolbar explaining what this kind is; omit it
   *  where the page header already says it. */
  blurb?: string | null;
  /** Primary action (New filter view / Import CBL / New collection). */
  action: React.ReactNode;
  /** Noun for copy: "list", "view", "collection". */
  noun: string;
  icon: LucideIcon;
  /** Card presentation: each view as a card in a grid per section. */
  renderCard?: (view: SavedViewView) => React.ReactNode;
  /** Rows presentation (a tree table) instead of cards: one column
   *  header, groups as folder rows, views as rows under them. The
   *  grouping, drag and drop, search, sort, undo and mutations are the
   *  same code either way; only the section and item shells differ. */
  rows?: BoardRowsPresentation;
  /** Extra items appended to each group's ⋯ menu (e.g. "Update lists in
   *  this group" on the reading-lists tab). `views` is every view in the
   *  group and the groups under it. */
  groupMenuExtra?: (
    group: BoardGroup,
    views: SavedViewView[],
  ) => React.ReactNode;
  /** Optional panel rendered between the toolbar and the board (the
   *  reading-lists tab mounts its updates panel here). */
  panel?: React.ReactNode;
  /** Card grid density: `compact` (Settings → Views, many small cards)
   *  or `large` (the `/collections` page: fixed 15rem cards, big covers). */
  size?: "compact" | "large";
};

/** What a rows-presentation row renderer is told about its row. */
export type BoardRowContext = {
  /** Indent level: 0 at the root, one more per enclosing group. */
  depth: number;
  density: BoardDensity;
  /** Rendering the lifted copy under the pointer (no live controls). */
  overlay: boolean;
};

export type BoardRowsPresentation = {
  /** Header cells after Name, each placed with a `ROW_CELL` class. A
   *  column with `sort` is a sort button for that key. */
  columns: ReadonlyArray<{
    key: string;
    label: string;
    cell: string;
    sort?: ViewSort;
  }>;
  /** The cells of one item row after the board's grip cell (lead, main,
   *  stats, flags, menu), placed with `ROW_CELL`. */
  renderRow: (view: SavedViewView, ctx: BoardRowContext) => React.ReactNode;
  /** The stat cells of a folder row: roll-ups over `views`, every view
   *  in the folder and the folders under it (unfiltered, also while
   *  searching). */
  renderGroupCells?: (
    group: BoardGroup,
    views: SavedViewView[],
  ) => React.ReactNode;
  /** Sort keys beyond the built-in ones, with their comparators (the
   *  views must be complete in memory). */
  sorts?: ReadonlyArray<{
    value: ViewSort;
    label: string;
    compare: ViewCompare;
  }>;
  /** Built-in sorts this board leaves out of its menu (the reading lists
   *  replace "Recently updated" with their own list date). */
  hiddenSorts?: ReadonlyArray<BuiltinViewSort>;
  /** `localStorage` key of the density choice. */
  densityKey: string;
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
  rows,
  groupMenuExtra,
  panel,
  size = "compact",
}: GroupedViewsBoardProps) {
  const groupsQ = useViewGroups(scope);
  const replaceLayout = useReplaceViewGroupLayout(scope);
  const createGroup = useCreateViewGroup();
  const updateGroup = useUpdateViewGroup(scope);
  const deleteGroup = useDeleteGroupWithUndo(scope, noun);

  const [sort, setSort] = React.useState<ViewSort>("custom");
  const [query, setQuery] = React.useState("");
  const [density, setDensity] = useBoardDensity(
    rows?.densityKey ?? "folio.viewBoard.density",
  );
  const extraSorts = rows?.sorts;
  const hiddenSorts = rows?.hiddenSorts;
  const sortOptions = React.useMemo(
    () => [
      ...VIEW_SORT_OPTIONS.filter(
        (o) => !(hiddenSorts ?? []).includes(o.value),
      ).map(({ value, label }) => ({ value: value as ViewSort, label })),
      ...(extraSorts ?? []).map(({ value, label }) => ({ value, label })),
    ],
    [extraSorts, hiddenSorts],
  );
  const compare = extraSorts?.find((s) => s.value === sort)?.compare;
  // The rows presentation sorts inside every group too; cards sort only
  // the ungrouped section. Groups always keep their arranged order.
  const sortInside = rows !== undefined;
  // Only the rows presentation offers nesting (new group inside, move a
  // group into another, drop a folder into a folder). Cards stay flat in
  // what they offer, but render and keep any nesting they're given.
  const nesting = rows !== undefined;
  // The stored hand-arranged board, whatever the sort: what a commit made
  // while sorted keeps (see `layoutKeepingCustom`).
  const customBoard = React.useMemo(
    () =>
      buildViewBoard(
        views,
        groupsQ.data?.groups ?? [],
        groupsQ.data?.ungrouped_ids ?? [],
        "custom",
      ),
    [views, groupsQ.data],
  );
  const serverBoard = React.useMemo(
    () => sortBoard(customBoard, sort, compare, { groups: sortInside }),
    [customBoard, sort, compare, sortInside],
  );
  const knownIds = React.useMemo(
    () => new Set(views.map((v) => v.id)),
    [views],
  );
  const sortLabel = sortOptions.find((o) => o.value === sort)?.label ?? "";
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
  // Where a held folder will land (rows), or why it can't, for the line /
  // ring on the target row and the label on the lifted row.
  const [folderDrag, setFolderDrag] = React.useState<FolderDragState | null>(
    null,
  );
  const [newGroupOpen, setNewGroupOpen] = React.useState(false);
  const [newGroupSeedView, setNewGroupSeedView] = React.useState<string | null>(
    null,
  );
  const [newGroupParent, setNewGroupParent] = React.useState<string | null>(
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

  const serverList = groupsQ.data;
  const commit = React.useCallback(
    (next: ViewBoard) => {
      const send = (layout: BoardLayout, shown: ViewBoard) => {
        setDraft(shown);
        // Ids this page doesn't know ride along in place, so a list filed
        // from another tab isn't moved out of its group by this save.
        replaceLayout.mutate(
          { scope, ...carryUnknownIds(layout, serverList, knownIds) },
          { onSettled: () => setDraft(null) },
        );
      };
      if (sort === "custom") {
        // What is shown is the hand arrangement: pin it as is.
        send(boardToLayout(next), next);
        return;
      }
      if (
        !sortInside &&
        sameMembers(next.ungrouped, serverBoard.ungrouped) &&
        !sameIds(next.ungrouped, serverBoard.ungrouped)
      ) {
        // Cards: a card dragged to a new spot inside the sorted ungrouped
        // section pins the order shown, and the sort switches to Custom so
        // it stays what the user sees.
        setSort("custom");
        send(boardToLayout(next), next);
        return;
      }
      // Any other move while sorted: never pin the sorted order. Each
      // section keeps its stored order, a newcomer goes to the end; the
      // tree (never sorted) comes from `next`.
      const layout = layoutKeepingCustom(customBoard, next, {
        groups: sortInside,
      });
      send(
        layout,
        sortBoard(boardFromLayout(next, layout), sort, compare, {
          groups: sortInside,
        }),
      );
    },
    [
      replaceLayout,
      scope,
      serverBoard,
      customBoard,
      sort,
      compare,
      sortInside,
      serverList,
      knownIds,
    ],
  );

  // ── actions shared with cards and rows (menu fallback for DnD) ──
  const actions = React.useMemo<ViewBoardActions>(
    () => ({
      groups: board.groups.map((g) => ({
        id: g.id,
        name: g.name,
        depth: groupDepth(board, g.id),
        path: groupPath(board, g.id),
      })),
      containerOf: (viewId) => containerOf(board, viewId),
      moveToGroup: (viewId, groupId) => {
        const next = moveView(board, viewId, groupId ?? UNGROUPED_ID);
        if (next !== board) commit(next);
      },
      newGroupWithView: (viewId) => {
        setNewGroupSeedView(viewId);
        setNewGroupParent(null);
        setNewGroupOpen(true);
      },
    }),
    [board, commit, setNewGroupSeedView, setNewGroupParent, setNewGroupOpen],
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

  const collisionDetection = React.useCallback<CollisionDetection>(
    (args) => {
      const active = parseDndId(args.active.id);
      if (active?.type === "group" && !nesting) {
        // Cards: groups reorder among themselves.
        return closestCenter({
          ...args,
          droppableContainers: args.droppableContainers.filter(
            (c) => parseDndId(c.id)?.type === "group",
          ),
        });
      }
      if (active?.type === "group") {
        return folderCollisions(args, subtreeIds(board, active.id));
      }
      // A held row: never the folder rows themselves (their sections
      // stand for them).
      const pool = {
        ...args,
        droppableContainers: args.droppableContainers.filter(
          (c) => parseDndId(c.id)?.type !== "head",
        ),
      };
      const within = pointerWithin(pool);
      const collisions = within.length > 0 ? within : rectIntersection(pool);
      // A card and its containers all collide; the card wins so the drop
      // index comes from a neighbour, not the whole section, and among
      // nested sections the innermost one wins.
      const card = collisions.find((c) => parseDndId(c.id)?.type === "view");
      if (card) return [card];
      const section = innermost(
        collisions.filter((c) => parseDndId(c.id)?.type === "group"),
        args.droppableRects,
      );
      return section ? [section] : collisions.slice(0, 1);
    },
    [board, nesting],
  );

  /** Where the held folder `id` lands when over `over` (rows). */
  const planFolderDrop = React.useCallback(
    (
      id: string,
      over: { id: UniqueIdentifier; rect: { top: number; height: number } },
      held: { top: number; height: number } | null,
    ): FolderDragState => {
      const o = parseDndId(over.id);
      let plan: FolderDrop | null = null;
      if (o?.type === "ungrouped") {
        plan = { parentId: null, index: undefined, target: null, zone: "into" };
      } else if (o?.type === "group") {
        plan = { parentId: o.id, index: undefined, target: o.id, zone: "into" };
      } else if (o?.type === "head") {
        const target = findGroup(board, o.id);
        if (target) {
          const center = held ? held.top + held.height / 2 : null;
          const rel =
            center === null || over.rect.height === 0
              ? 0.5
              : (center - over.rect.top) / over.rect.height;
          const zone = rel < 0.3 ? "before" : rel > 0.7 ? "after" : "into";
          if (zone === "into") {
            plan = {
              parentId: target.id,
              index: undefined,
              target: o.id,
              zone,
            };
          } else {
            const siblings = childGroups(board, target.parentId).filter(
              (g) => g.id !== id,
            );
            const at = siblings.findIndex((g) => g.id === target.id);
            plan = {
              parentId: target.parentId,
              index: at + (zone === "after" ? 1 : 0),
              target: o.id,
              zone,
            };
          }
        }
      }
      if (!plan) return { plan: null, refusal: null };
      const refusal = groupMoveRefusal(board, id, plan.parentId);
      return refusal ? { plan: null, refusal } : { plan, refusal: null };
    },
    [board],
  );

  function onDragStart(ev: DragStartEvent) {
    const a = parseDndId(ev.active.id);
    setDragFrom(a?.type === "view" ? containerOf(board, a.id) : null);
    setActiveId(ev.active.id);
    setFolderDrag(null);
    setDraft(board);
  }

  function onDragMove(ev: DragMoveEvent) {
    const a = parseDndId(ev.active.id);
    if (!nesting || a?.type !== "group") return;
    const next = ev.over
      ? planFolderDrop(a.id, ev.over, ev.active.rect.current.translated)
      : { plan: null, refusal: null };
    setFolderDrag((prev) =>
      JSON.stringify(prev) === JSON.stringify(next) ? prev : next,
    );
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
    setFolderDrag(null);
    suppressClickUntil.current = Date.now() + 200;
  }

  function onDragEnd(ev: DragEndEvent) {
    const { active, over } = ev;
    const a = parseDndId(active.id);
    const held = folderDrag;
    finishDrag();
    if (!over || !a) {
      setDraft(null);
      return;
    }
    if (a.type === "group" && nesting) {
      // Rows: the folder lands where its lifted row said; a refused drop
      // (into itself, too deep) changes nothing.
      const { plan } =
        held ??
        planFolderDrop(a.id, over, active.rect.current.translated ?? null);
      const next = plan
        ? moveGroupTo(board, a.id, plan.parentId, plan.index)
        : board;
      if (next === board) setDraft(null);
      else commit(next);
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
        // Same-section reorder (a group or Ungrouped). Rows sorted by a
        // column have no hand order to change: only moves between
        // sections count then.
        if (sortInside && sort !== "custom") {
          // The draft may already hold a move into this section from
          // `onDragOver`: that move counts, a reorder does not.
          if (dragFrom !== null && dragFrom !== overContainer) commit(board);
          else setDraft(null);
          return;
        }
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
        ...(newGroupParent ? { parent_id: newGroupParent } : {}),
      },
      {
        onSuccess: () => {
          setNewGroupOpen(false);
          setNewGroupSeedView(null);
          setNewGroupParent(null);
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

  // The rows board is a query container (`@container/board`), and
  // container queries contain layout: a fixed-position overlay inside it
  // would be placed against the board, not the viewport. Rows render the
  // overlay in a portal on `body` instead (client only).
  const mounted = React.useSyncExternalStore(
    noopSubscribe,
    () => true,
    () => false,
  );
  const portal = (node: React.ReactNode) =>
    rows && mounted ? createPortal(node, document.body) : node;

  // Screen-reader announcements in names (with the level of a group),
  // not dnd ids.
  const groupName = (id: string): string => {
    const g = findGroup(board, id);
    if (!g) return "a group";
    return nesting
      ? `group ${g.name}, level ${groupDepth(board, id)}`
      : `group ${g.name}`;
  };
  const containerName = (container: string | null): string =>
    container === null
      ? "no drop area"
      : container === UNGROUPED_ID
        ? nesting
          ? "the top level, in no group"
          : "Ungrouped"
        : groupName(container);
  const nameOf = (id: UniqueIdentifier | undefined): string => {
    const p = id === undefined ? null : parseDndId(id);
    if (!p) return "";
    if (p.type === "ungrouped") return containerName(UNGROUPED_ID);
    if (p.type === "group" || p.type === "head") return groupName(p.id);
    return views.find((v) => v.id === p.id)?.name ?? `a ${noun}`;
  };
  const describePlan = (state: FolderDragState): string => {
    if (!state.plan) {
      return state.refusal === "depth"
        ? `can't go there: groups nest at most ${MAX_GROUP_DEPTH} levels`
        : state.refusal === "self"
          ? "can't go inside itself"
          : "is not over a drop area";
    }
    const { plan } = state;
    if (plan.zone === "into") {
      return plan.parentId === null
        ? "will move to the top level"
        : `will move into ${groupName(plan.parentId)}`;
    }
    return `will move ${plan.zone} ${plan.target ? groupName(plan.target) : "the top level"}`;
  };
  const announcements: Announcements = {
    onDragStart: ({ active }) => `Picked up ${nameOf(active.id)}.`,
    onDragOver: ({ active, over }) => {
      const a = parseDndId(active.id);
      if (!over) return `${nameOf(active.id)} is no longer over a drop area.`;
      if (a?.type === "group" && nesting) {
        return `${nameOf(active.id)} ${describePlan(
          planFolderDrop(a.id, over, active.rect.current.translated),
        )}.`;
      }
      if (a?.type === "view") {
        return `${nameOf(active.id)} is over ${containerName(resolveContainer(over.id))}.`;
      }
      return `${nameOf(active.id)} is over ${nameOf(over.id)}.`;
    },
    onDragEnd: ({ active, over }) => {
      const a = parseDndId(active.id);
      if (!over) return `${nameOf(active.id)} was dropped.`;
      if (a?.type === "group" && nesting) {
        const state = planFolderDrop(
          a.id,
          over,
          active.rect.current.translated,
        );
        return state.plan
          ? `${nameOf(active.id)} was moved: it ${describePlan(state).replace(/^will move/, "now sits")}.`
          : `${nameOf(active.id)} was put back: it ${describePlan(state)}.`;
      }
      // Rows sorted by a column ignore a drop inside the row's own
      // section (see `onDragEnd`): say so rather than "dropped on".
      if (
        sortInside &&
        sort !== "custom" &&
        a?.type === "view" &&
        resolveContainer(over.id) === dragFrom
      ) {
        return `Sorted by ${sortLabel.toLowerCase()}. Order unchanged.`;
      }
      if (a?.type === "view") {
        return `${nameOf(active.id)} was dropped in ${containerName(resolveContainer(over.id))}.`;
      }
      return `${nameOf(active.id)} was dropped on ${nameOf(over.id)}.`;
    },
    onDragCancel: ({ active }) =>
      `Dragging was cancelled. ${nameOf(active.id)} was put back.`,
  };
  const renderView = renderCard ?? renderNothing;

  /** The per-group callbacks both presentations wire up the same way. */
  function groupHandlers(id: string): GroupHandlers {
    const group = findGroup(board, id)!;
    const siblings = childGroups(board, group.parentId);
    const at = siblings.findIndex((g) => g.id === id);
    const depth = groupDepth(board, id);
    const inside = subtreeIds(board, id);
    const moveTargets: MoveTarget[] = nesting
      ? [
          {
            id: null,
            label: "Top level",
            path: "Top level",
            depth: 0,
            current: group.parentId === null,
            refusal: null,
          },
          ...board.groups
            .filter((g) => !inside.has(g.id))
            .map((g) => ({
              id: g.id,
              label: g.name,
              path: groupPath(board, g.id).join(" / "),
              depth: groupDepth(board, g.id),
              current: group.parentId === g.id,
              refusal: groupMoveRefusal(board, id, g.id),
            })),
        ]
      : [];
    return {
      depth,
      isFirst: at <= 0,
      isLast: at === siblings.length - 1,
      nesting,
      canNestInside: canNestInside(board, id),
      moveTargets,
      onRename: (name: string) => updateGroup.mutate({ id, name }),
      onToggleCollapsed: () =>
        updateGroup.mutate({ id, collapsed: !group.collapsed }),
      onMove: (dir: -1 | 1) => {
        const next = moveGroupAmongSiblings(board, id, dir);
        if (next !== board) commit(next);
      },
      onMoveTo: (parentId: string | null) => {
        const next = moveGroupTo(board, id, parentId);
        if (next !== board) commit(next);
      },
      onNewInside: () => {
        setNewGroupSeedView(null);
        setNewGroupParent(id);
        setNewGroupOpen(true);
      },
      // Undo restores from the server's stored tree (captured inside the
      // hook), never from the possibly sorted board on screen.
      onDelete: () =>
        deleteGroup(
          findGroup(customBoard, id) ?? group,
          childGroups(board, id).length > 0,
        ),
      menuExtra: groupMenuExtra?.(group, subtreeViews(board, id)),
    };
  }

  const heldFolderLabel =
    activeGroup && nesting && folderDrag
      ? folderDrag.plan
        ? folderDrag.plan.zone === "into"
          ? folderDrag.plan.parentId === null
            ? "To the top level"
            : `Into ${groupPath(board, folderDrag.plan.parentId).join(" / ")}`
          : `${folderDrag.plan.zone === "before" ? "Before" : "After"} ${findGroup(board, folderDrag.plan.target ?? "")?.name ?? ""}`
        : folderDrag.refusal === "depth"
          ? `Too deep: at most ${MAX_GROUP_DEPTH} levels`
          : folderDrag.refusal === "self"
            ? "Can't go inside itself"
            : null
      : null;
  const heldRowLabel =
    activeView && rows && heldIn
      ? heldIn === UNGROUPED_ID
        ? showGroups
          ? "Not in a group"
          : null
        : `In ${groupPath(board, heldIn).join(" / ")}`
      : null;

  return (
    <ViewBoardActionsContext.Provider value={actions}>
      <CardGridClassContext.Provider value={CARD_GRID_CLASS[size]}>
        <div className={cn("space-y-4", rows && BOARD_CONTAINER)}>
          {/* Toolbar: blurb + search + sort + new group + primary action */}
          <div className="flex flex-wrap items-center justify-between gap-2">
            {blurb ? (
              <p className="text-muted-foreground text-sm">{blurb}</p>
            ) : null}
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
                      aria-label={rows ? "Sort" : "Sort ungrouped"}
                      title={
                        rows
                          ? `Order of the ${plural(noun, 2)} in every group and outside them. Groups keep their own order.`
                          : "Order of the ungrouped section. Dragging a card pins the custom order."
                      }
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent align="end">
                      {sortOptions.map((o) => (
                        <SelectItem key={o.value} value={o.value}>
                          {o.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  {rows ? (
                    <DensityToggle value={density} onChange={setDensity} />
                  ) : null}
                  <Button
                    type="button"
                    variant="outline"
                    className="h-9"
                    onClick={() => {
                      setNewGroupSeedView(null);
                      setNewGroupParent(null);
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
                  <BoardToolbarControlsSkeleton density={rows !== undefined} />
                </div>
              ) : null}
              {action}
            </div>
          </div>

          {panel}

          {loading || groupsQ.isLoading ? (
            rows ? (
              <LoadingRegion label={`Loading ${plural(noun, 2)}…`}>
                <ViewRowsSkeleton />
              </LoadingRegion>
            ) : (
              <BoardSkeleton />
            )
          ) : total === 0 ? (
            <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-6 text-sm">
              {empty}
            </div>
          ) : (
            <DndContext
              sensors={sensors}
              accessibility={{ announcements }}
              collisionDetection={collisionDetection}
              onDragStart={onDragStart}
              onDragMove={onDragMove}
              onDragOver={onDragOver}
              onDragEnd={onDragEnd}
              onDragCancel={onDragCancel}
            >
              {rows ? (
                <RowsTable
                  presentation={rows}
                  density={density}
                  board={board}
                  visibleBoard={visibleBoard}
                  sort={sort}
                  onSort={setSort}
                  sortNote={
                    sort === "custom"
                      ? null
                      : `Sorted by ${sortLabel.toLowerCase()}: dragging moves a ${noun} between groups. Choose Custom order to arrange by hand.`
                  }
                  searching={searching}
                  noun={noun}
                  dragActive={activeId !== null}
                  heldFolder={activeGroup?.id ?? null}
                  dropTarget={dropTarget}
                  folderDrop={folderDrag?.plan ?? null}
                  suppressClickUntil={suppressClickUntil}
                  groupHandlers={groupHandlers}
                />
              ) : (
                /* While dragging, cards under the pointer must not react
                  (hover wash, cover fan, tint): the slot and the target
                  group's ring are the only feedback. Collision detection
                  uses rects, not pointer events. */
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
                    {visibleBoard.groups.map((group) => (
                      <GroupSection
                        key={group.id}
                        group={group}
                        // Cards are flat: a nested group (made on the
                        // reading lists page) shows its whole path.
                        title={groupPath(board, group.id).join(" / ")}
                        totalInGroup={
                          findGroup(board, group.id)?.views.length ?? 0
                        }
                        forceOpen={searching}
                        noun={noun}
                        dragActive={activeId !== null}
                        isDropTarget={dropTarget === group.id}
                        suppressClickUntil={suppressClickUntil}
                        renderCard={renderView}
                        {...groupHandlers(group.id)}
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
                    renderCard={renderView}
                  />
                </div>
              )}

              {portal(
                <DragOverlay dropAnimation={reduced ? null : DROP_ANIMATION}>
                  {activeView && rows ? (
                    // The held row at its own width, in a query container of
                    // its own (the overlay lives in a portal, outside the
                    // board's), so it keeps the table's tier.
                    <div className={BOARD_CONTAINER}>
                      <div
                        data-density={density}
                        className={cn(
                          ROW_GRID,
                          ROW_PAD,
                          "bg-popover text-popover-foreground border-border pointer-events-none rounded-md border shadow-xl",
                          DRAG_LIFT_CLASS,
                        )}
                        style={{ transition: LIFT_TRANSITION }}
                      >
                        <span
                          className={cn(
                            "text-muted-foreground justify-center",
                            ROW_CELL.grip,
                          )}
                        >
                          <GripVertical className="size-4" aria-hidden="true" />
                        </span>
                        {rows.renderRow(activeView, {
                          depth: 0,
                          density,
                          overlay: true,
                        })}
                      </div>
                      {heldRowLabel ? <DropLabel label={heldRowLabel} /> : null}
                    </div>
                  ) : activeView ? (
                    // A lifted copy of the card itself, at its own size, so
                    // what you hold is what lands.
                    <div
                      className={cn(
                        "pointer-events-none rounded-lg",
                        DRAG_LIFT_CLASS,
                      )}
                      style={{ transition: LIFT_TRANSITION }}
                    >
                      {renderView(activeView)}
                    </div>
                  ) : activeGroup ? (
                    <div>
                      <DragGhost
                        icon={nesting ? Folder : GripVertical}
                        label={activeGroup.name}
                        meta={`${subtreeViews(board, activeGroup.id).length} ${plural(noun, subtreeViews(board, activeGroup.id).length)}`}
                      />
                      {heldFolderLabel ? (
                        <DropLabel
                          label={heldFolderLabel}
                          refused={folderDrag?.plan === null}
                        />
                      ) : null}
                    </div>
                  ) : null}
                </DragOverlay>,
              )}
            </DndContext>
          )}
        </div>

        <NewGroupDialog
          open={newGroupOpen}
          onOpenChange={(o) => {
            setNewGroupOpen(o);
            if (!o) {
              setNewGroupSeedView(null);
              setNewGroupParent(null);
            }
          }}
          seedViewName={
            newGroupSeedView
              ? views.find((v) => v.id === newGroupSeedView)?.name
              : undefined
          }
          parentPath={
            newGroupParent
              ? groupPath(board, newGroupParent).join(" / ")
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

type FolderDragState = {
  plan: FolderDrop | null;
  refusal: GroupMoveRefusal | null;
};

/** One "Move to…" destination of a group's menu. */
type MoveTarget = {
  /** `null`: the top level. */
  id: string | null;
  label: string;
  /** The names from the top level down (the item's tooltip). */
  path: string;
  /** 0 for the top level, else the destination group's level. */
  depth: number;
  current: boolean;
  refusal: GroupMoveRefusal | null;
};

/** The collision of a held folder (rows): the folder row under the
 *  pointer (its top / middle / bottom picks before / into / after),
 *  else the innermost section, else the root. Nothing while the pointer
 *  is over the held folder's own subtree. Keyboard drags (no pointer)
 *  take the nearest folder row. */
function folderCollisions(
  args: Parameters<CollisionDetection>[0],
  inside: ReadonlySet<string>,
) {
  const folderish = args.droppableContainers.filter((c) => {
    const t = parseDndId(c.id)?.type;
    return t === "head" || t === "group" || t === "ungrouped";
  });
  if (!args.pointerCoordinates) {
    return closestCenter({
      ...args,
      droppableContainers: folderish.filter((c) => {
        const p = parseDndId(c.id);
        return p?.type === "head" && !inside.has(p.id);
      }),
    });
  }
  const hits = pointerWithin({ ...args, droppableContainers: folderish });
  const own = (c: { id: UniqueIdentifier }) => {
    const p = parseDndId(c.id);
    return (p?.type === "head" || p?.type === "group") && inside.has(p.id);
  };
  if (hits.some(own)) return [];
  const head = hits.find((c) => parseDndId(c.id)?.type === "head");
  if (head) return [head];
  const section = innermost(
    hits.filter((c) => parseDndId(c.id)?.type === "group"),
    args.droppableRects,
  );
  if (section) return [section];
  return hits.filter((c) => parseDndId(c.id)?.type === "ungrouped").slice(0, 1);
}

/** The smallest of `collisions` by area: among nested sections under the
 *  pointer, the innermost one. */
function innermost<T extends { id: UniqueIdentifier }>(
  collisions: T[],
  rects: Parameters<CollisionDetection>[0]["droppableRects"],
): T | null {
  let best: T | null = null;
  let bestArea = Infinity;
  for (const c of collisions) {
    const r = rects.get(c.id);
    const area = r ? r.width * r.height : Infinity;
    if (best === null || area < bestArea) {
      best = c;
      bestArea = area;
    }
  }
  return best;
}

/** Under the lifted row or folder: where it will land. */
function DropLabel({ label, refused }: { label: string; refused?: boolean }) {
  return (
    <p
      className={cn(
        "bg-popover border-border mt-1 inline-block max-w-xs truncate rounded-md border px-2 py-0.5 text-xs shadow-md",
        refused ? "text-destructive" : "text-muted-foreground",
      )}
    >
      {label}
    </p>
  );
}

const noopSubscribe = () => () => {};

function renderNothing(): React.ReactNode {
  return null;
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

function sameMembers(a: readonly SavedViewView[], b: readonly SavedViewView[]) {
  const ids = new Set(a.map((v) => v.id));
  return a.length === b.length && b.every((v) => ids.has(v.id));
}

// ──────────────────── group section ────────────────────

function GroupSection({
  group,
  title,
  totalInGroup,
  forceOpen,
  noun,
  dragActive,
  isDropTarget,
  suppressClickUntil,
  renderCard,
  ...handlers
}: {
  group: BoardGroup;
  /** The group's name, or its path when it is nested. */
  title: string;
  /** Unfiltered member count (the badge), vs `group.views` which may be
   *  narrowed by search. */
  totalInGroup: number;
  forceOpen: boolean;
  noun: string;
  dragActive: boolean;
  /** A held card from another section will land here. */
  isDropTarget: boolean;
  suppressClickUntil: React.MutableRefObject<number>;
  renderCard: (view: SavedViewView) => React.ReactNode;
} & GroupHandlers) {
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
  const { onRename, onToggleCollapsed } = handlers;
  const [editing, setEditing] = React.useState(false);
  const open = forceOpen || !group.collapsed;
  const panelId = React.useId();
  // Highlight when a *card* hovers this section (not another group).
  return (
    <section
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition }}
      aria-label={`Group ${title}`}
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
            title={title}
          >
            {title}
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
        <GroupMenu
          name={group.name}
          collapsed={group.collapsed}
          onStartRename={() => setEditing(true)}
          {...handlers}
        />
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

/** A group's ⋯ menu: rename, (rows) new group inside and move to…,
 *  collapse, move up / down among its siblings, the board's extras (the
 *  reading lists' "Update these lists"), delete. Shared by the card
 *  section header and the folder row. Destinations the depth cap rules
 *  out stay listed, disabled, with the reason. */
function GroupMenu({
  name,
  collapsed,
  onStartRename,
  isFirst,
  isLast,
  nesting,
  canNestInside,
  moveTargets,
  onToggleCollapsed,
  onMove,
  onMoveTo,
  onNewInside,
  onDelete,
  menuExtra,
}: {
  name: string;
  collapsed: boolean;
  onStartRename: () => void;
} & GroupHandlers) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          aria-label={`Actions for group ${name}`}
          className="text-muted-foreground hover:text-foreground size-8 shrink-0"
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-60">
        <DropdownMenuItem onSelect={onStartRename}>
          <Pencil className="size-4" /> Rename
        </DropdownMenuItem>
        {nesting ? (
          <>
            <DropdownMenuItem onSelect={onNewInside} disabled={!canNestInside}>
              <FolderPlus className="size-4" />
              <MenuItemText
                label="New group inside"
                reason={
                  canNestInside
                    ? null
                    : `Groups nest at most ${MAX_GROUP_DEPTH} levels`
                }
              />
            </DropdownMenuItem>
            <DropdownMenuResponsiveSub
              icon={<FolderInput className="size-4" />}
              label="Move to…"
            >
              {moveTargets.map((t) => (
                <DropdownMenuItem
                  key={t.id ?? "top"}
                  disabled={t.current || t.refusal !== null}
                  onSelect={() => onMoveTo(t.id)}
                  title={t.path}
                >
                  <span
                    aria-hidden="true"
                    className="inline-flex shrink-0 justify-end"
                    style={{
                      width: `${1 + Math.max(0, t.depth - 1) * 0.75}rem`,
                    }}
                  >
                    {t.current ? <Check className="size-4" /> : null}
                  </span>
                  <MenuItemText
                    label={t.label}
                    reason={
                      t.refusal === "depth"
                        ? `Too deep: at most ${MAX_GROUP_DEPTH} levels`
                        : null
                    }
                  />
                </DropdownMenuItem>
              ))}
            </DropdownMenuResponsiveSub>
          </>
        ) : null}
        <DropdownMenuItem onSelect={onToggleCollapsed}>
          {collapsed ? (
            <>
              <ChevronDown className="size-4" /> Expand
            </>
          ) : (
            <>
              <ChevronRight className="size-4" /> Collapse
            </>
          )}
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
  );
}

/** A menu item's label with, when it is disabled, why (a second line; a
 *  disabled item can't show a tooltip). */
function MenuItemText({
  label,
  reason,
}: {
  label: string;
  reason: string | null;
}) {
  return (
    <span className="flex min-w-0 flex-col">
      <span className="truncate">{label}</span>
      {reason ? (
        <span className="text-muted-foreground text-xs">{reason}</span>
      ) : null}
    </span>
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
  const { onKeyDown, surface } = dragSurface(listeners, suppressClickUntil);

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
      <div role="presentation" className="contents" {...surface}>
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

/** Elements inside a drag surface that keep their plain clicks. A grip
 *  marked `data-drag-handle` is a button but still starts a drag. */
const NO_DRAG =
  "button:not([data-drag-handle]), [role=menuitem], input, select, textarea";

/**
 * Split a sortable's listeners into the keyboard half (for the grip,
 * which carries the sortable ARIA attributes) and pointer handlers for a
 * presentational wrapper, so the whole card / row is a pointer drag
 * surface except its buttons and fields. Clicks that land right after a
 * drop are swallowed so a dropped link doesn't navigate.
 */
function dragSurface(
  listeners: ReturnType<typeof useSortable>["listeners"],
  suppressClickUntil: React.MutableRefObject<number>,
) {
  const { onKeyDown, ...pointer } = (listeners ?? {}) as Record<
    string,
    (e: React.SyntheticEvent) => void
  >;
  // React events from a portal (the pin dialog, a menu) bubble through
  // the component tree into this surface; only events from its own DOM
  // count.
  const inside = (e: React.SyntheticEvent) =>
    (e.currentTarget as Element).contains(e.target as Node);
  const guard =
    (name: "onPointerDown" | "onMouseDown" | "onTouchStart") =>
    (e: React.SyntheticEvent) => {
      if (!inside(e)) return;
      // Buttons / menu items keep plain clicks; everything else starts a
      // drag after the sensor's distance (mouse) or delay (touch).
      if ((e.target as Element).closest(NO_DRAG)) return;
      pointer[name]?.(e);
    };
  return {
    onKeyDown,
    surface: {
      // Native anchor/image drag would fight the pointer sensor.
      onDragStart: (e: React.DragEvent) => {
        if (inside(e)) e.preventDefault();
      },
      onPointerDown: guard("onPointerDown"),
      onMouseDown: guard("onMouseDown"),
      onTouchStart: guard("onTouchStart"),
      onClickCapture: (e: React.MouseEvent) => {
        if (inside(e) && Date.now() < suppressClickUntil.current) {
          e.preventDefault();
          e.stopPropagation();
        }
      },
    },
  };
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

// ──────────────────── rows presentation (tree table) ────────────────────

function DensityToggle({
  value,
  onChange,
}: {
  value: BoardDensity;
  onChange: (next: BoardDensity) => void;
}) {
  return (
    // Density only changes the wide table; below it rows always use the
    // two-line form, so the control shows in the wide tier only.
    <ToggleGroup
      type="single"
      variant="outline"
      spacing={0}
      value={value}
      onValueChange={(v) => {
        if (v === "compact" || v === "comfortable") onChange(v);
      }}
      aria-label="Row density"
      className="folio-control hidden @min-[42rem]/board:flex"
    >
      <ToggleGroupItem
        value="compact"
        aria-label="Compact rows"
        title="Compact: one line per row"
      >
        <Rows4 aria-hidden="true" />
      </ToggleGroupItem>
      <ToggleGroupItem
        value="comfortable"
        aria-label="Comfortable rows"
        title="Comfortable: the description under the name"
      >
        <Rows2 aria-hidden="true" />
      </ToggleGroupItem>
    </ToggleGroup>
  );
}

type GroupHandlers = {
  /** Level of the group: 1 at the top. */
  depth: number;
  /** First / last among its siblings. */
  isFirst: boolean;
  isLast: boolean;
  /** The rows presentation: nesting moves are offered. */
  nesting: boolean;
  /** A group can be made inside this one (the depth cap allows it). */
  canNestInside: boolean;
  /** "Move to…" destinations: the top level and every group outside
   *  this one's subtree, in tree order. */
  moveTargets: MoveTarget[];
  onRename: (name: string) => void;
  onToggleCollapsed: () => void;
  onMove: (dir: -1 | 1) => void;
  onMoveTo: (parentId: string | null) => void;
  onNewInside: () => void;
  onDelete: () => void;
  menuExtra?: React.ReactNode;
};

/**
 * The rows presentation: one column header, then the board's tree
 * (`boardRows`) — folder rows with their subfolders and views one level
 * in, at any depth, then the ungrouped views at the root. A list with a
 * visual header rather than a `<table>`: rows are sortable `<li>`s that
 * move under transforms, a folder is a `<section>` the held row can drop
 * into (nested in its parent folder's list), and below `sm` a row
 * restacks onto two grid lines, none of which a table's row model
 * allows. Each value carries its label for assistive tech in the row
 * renderer (visually hidden where the header supplies it).
 */
function RowsTable({
  presentation,
  density,
  board,
  visibleBoard,
  sort,
  onSort,
  sortNote,
  searching,
  noun,
  dragActive,
  heldFolder,
  dropTarget,
  folderDrop,
  suppressClickUntil,
  groupHandlers,
}: {
  presentation: BoardRowsPresentation;
  density: BoardDensity;
  board: ViewBoard;
  visibleBoard: ViewBoard;
  sort: ViewSort;
  onSort: (sort: ViewSort) => void;
  /** Set while a column sort is on: what a drag can still do. */
  sortNote: string | null;
  searching: boolean;
  noun: string;
  dragActive: boolean;
  /** The folder being dragged, if any. */
  heldFolder: string | null;
  dropTarget: string | null;
  folderDrop: FolderDrop | null;
  suppressClickUntil: React.MutableRefObject<number>;
  groupHandlers: (id: string) => GroupHandlers;
}) {
  const tree = React.useMemo(() => boardRows(visibleBoard), [visibleBoard]);
  const folders = tree.filter(
    (r): r is Extract<BoardRow, { type: "group" }> => r.type === "group",
  );
  const rootViews = tree.filter(
    (r): r is Extract<BoardRow, { type: "view" }> => r.type === "view",
  );
  const nothingVisible =
    searching &&
    rootViews.length === 0 &&
    visibleBoard.groups.every((g) => g.views.length === 0);
  const shared = {
    board,
    forceOpen: searching,
    noun,
    dragActive,
    heldFolder,
    dropTarget,
    folderDrop,
    suppressClickUntil,
    presentation,
    density,
    sortNote,
    groupHandlers,
  };
  return (
    <div
      data-density={density}
      className={cn(
        "border-border/60 bg-card/40 rounded-lg border",
        // While dragging, rows under the pointer must not react; the slot
        // and the target folder's ring are the only feedback.
        dragActive && "select-none [&_[data-board-card]]:pointer-events-none",
      )}
    >
      <RowsHeader
        columns={presentation.columns}
        sort={sort}
        onSort={onSort}
        noun={noun}
      />
      {folders.map((node) => (
        <FolderSection key={node.id} node={node} {...shared} />
      ))}
      <UngroupedRows
        rows={rootViews}
        hasGroups={board.groups.length > 0}
        noun={noun}
        nothingVisible={nothingVisible}
        dragActive={dragActive}
        isDropTarget={
          dropTarget === UNGROUPED_ID ||
          (folderDrop !== null &&
            folderDrop.parentId === null &&
            folderDrop.zone === "into")
        }
        heldFolder={heldFolder !== null}
        suppressClickUntil={suppressClickUntil}
        presentation={presentation}
        density={density}
        sortNote={sortNote}
      />
    </div>
  );
}

/** The table's one column header. Labels with a sort key are buttons:
 *  press to sort by that column, press the active one again for the
 *  hand-arranged order. A sort orders the rows inside every group and
 *  outside them; groups keep their arranged order. The narrow tier has
 *  no header: the sort menu in the toolbar covers it. */
function RowsHeader({
  columns,
  sort,
  onSort,
  noun,
}: {
  columns: BoardRowsPresentation["columns"];
  sort: ViewSort;
  onSort: (sort: ViewSort) => void;
  noun: string;
}) {
  const cells: Array<{
    key: string;
    label: string;
    cell: string;
    sort?: ViewSort;
  }> = [
    {
      key: "name",
      label: "Name",
      cell: "@min-[30rem]/board:col-start-2 @min-[30rem]/board:col-span-2 @min-[30rem]/board:row-start-1",
      sort: "name",
    },
    ...columns,
  ];
  return (
    <div
      data-board-header=""
      className={cn(
        ROW_GRID,
        "text-muted-foreground border-border/60 hidden border-b px-2 py-1.5 text-xs font-medium @min-[30rem]/board:grid @min-[30rem]/board:px-3",
      )}
    >
      {cells.map((c) => {
        const active = c.sort !== undefined && sort === c.sort;
        return (
          <div key={c.key} className={c.cell}>
            {c.sort !== undefined ? (
              <button
                type="button"
                aria-pressed={active}
                onClick={() => onSort(active ? "custom" : c.sort!)}
                title={
                  active
                    ? `Sorted by ${c.label.toLowerCase()}. Press again for your custom order.`
                    : `Sort ${plural(noun, 2)} by ${c.label.toLowerCase()}`
                }
                className={cn(
                  "folio-tap hover:text-foreground focus-visible:ring-ring inline-flex items-center gap-1 rounded focus-visible:ring-2 focus-visible:outline-none",
                  active && "text-foreground",
                )}
              >
                {c.label}
                {active ? (
                  <ArrowDown className="size-3" aria-hidden="true" />
                ) : null}
              </button>
            ) : (
              <span>{c.label}</span>
            )}
          </div>
        );
      })}
    </div>
  );
}

const ROW_LINE = "border-border/50 border-t";

/** A folder row and, under it, its subfolders and then its rows, at any
 *  depth. The section is the folder's drop target for a held row (and
 *  "into" for a held folder); the folder row itself is a second target,
 *  whose top / bottom edge puts a held folder before / after it. The
 *  folder row is the pointer drag surface, its grip the keyboard handle.
 *  Collapsing hides everything under it. */
function FolderSection({
  node,
  board,
  forceOpen,
  noun,
  dragActive,
  heldFolder,
  dropTarget,
  folderDrop,
  suppressClickUntil,
  presentation,
  density,
  sortNote,
  groupHandlers,
}: {
  node: Extract<BoardRow, { type: "group" }>;
  /** The whole board (unfiltered): roll-ups and counts while a search
   *  narrows `node.children`. */
  board: ViewBoard;
  forceOpen: boolean;
  noun: string;
  dragActive: boolean;
  heldFolder: string | null;
  dropTarget: string | null;
  folderDrop: FolderDrop | null;
  suppressClickUntil: React.MutableRefObject<number>;
  presentation: BoardRowsPresentation;
  density: BoardDensity;
  sortNote: string | null;
  groupHandlers: (id: string) => GroupHandlers;
}) {
  const group = node.group;
  const handlers = groupHandlers(group.id);
  const { onRename, onToggleCollapsed, depth } = handlers;
  const {
    attributes,
    listeners,
    setNodeRef: setDragRef,
    setActivatorNodeRef,
    isDragging,
  } = useDraggable({
    id: groupDndId(group.id),
    data: { type: "group" },
  });
  const { setNodeRef: setDropRef } = useDroppable({
    id: groupDndId(group.id),
  });
  const { setNodeRef: setHeadRef } = useDroppable({
    id: headDndId(group.id),
  });
  // The folder row is what is held (its rect, not the whole section's,
  // is what the before / into / after zones are measured with) and also
  // the folder row's own drop target.
  const setRowRef = React.useCallback(
    (el: HTMLElement | null) => {
      setDragRef(el);
      setHeadRef(el);
    },
    [setDragRef, setHeadRef],
  );
  const { onKeyDown, surface } = dragSurface(listeners, suppressClickUntil);
  const [editing, setEditing] = React.useState(false);
  const open = forceOpen || !group.collapsed;
  const panelId = React.useId();
  const all = subtreeViews(board, group.id);
  const count = all.length;
  const FolderIcon = open ? FolderOpen : Folder;
  const subfolders = node.children.filter(
    (c): c is Extract<BoardRow, { type: "group" }> => c.type === "group",
  );
  const lists = node.children.filter(
    (c): c is Extract<BoardRow, { type: "view" }> => c.type === "view",
  );
  const into =
    dropTarget === group.id ||
    (folderDrop?.zone === "into" && folderDrop.parentId === group.id);
  const edge =
    folderDrop?.target === group.id && folderDrop.zone !== "into"
      ? folderDrop.zone
      : null;
  const label = depth > 1 ? `${group.name}, level ${depth}` : group.name;
  return (
    <section
      ref={setDropRef}
      aria-label={`Group ${label}`}
      data-folder-depth={depth}
      className={cn(
        "[&:not(:first-child)]:border-border/50 [&:not(:first-child)]:border-t",
        isDragging && "opacity-50",
        into && "bg-primary/5 ring-primary/60 ring-2 ring-inset",
      )}
    >
      <div
        ref={setRowRef}
        className={cn(
          ROW_GRID,
          ROW_PAD,
          "group/row bg-muted/30",
          edge === "before" && "shadow-[inset_0_2px_0_0_var(--color-primary)]",
          edge === "after" && "shadow-[inset_0_-2px_0_0_var(--color-primary)]",
        )}
        style={{ "--row-depth": node.depth } as React.CSSProperties}
      >
        <div role="presentation" className="contents" {...surface}>
          <button
            type="button"
            ref={setActivatorNodeRef}
            {...attributes}
            onKeyDown={onKeyDown}
            data-drag-handle=""
            aria-label={`Drag group ${group.name}`}
            className={cn(GRIP_CLASS, ROW_CELL.grip)}
          >
            <GripVertical className="size-4" aria-hidden="true" />
          </button>
          <div className={cn(ROW_CELL.lead, "row-span-1 self-center")}>
            <button
              type="button"
              onClick={onToggleCollapsed}
              aria-expanded={open}
              aria-controls={panelId}
              disabled={forceOpen}
              aria-label={
                open ? `Collapse ${group.name}` : `Expand ${group.name}`
              }
              className="folio-tap text-muted-foreground hover:text-foreground focus-visible:ring-ring inline-flex size-7 shrink-0 items-center justify-center rounded focus-visible:ring-2 focus-visible:outline-none disabled:opacity-50"
            >
              <ChevronRight
                aria-hidden="true"
                className={cn(
                  "size-4 transition-transform motion-reduce:transition-none",
                  open && "rotate-90",
                )}
              />
            </button>
            <FolderIcon
              className="text-muted-foreground size-4 shrink-0"
              aria-hidden="true"
            />
          </div>
          <div className={cn(ROW_CELL.main, "flex items-baseline gap-2")}>
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
                aria-expanded={open}
                aria-controls={panelId}
                className="focus-visible:ring-ring min-w-0 rounded text-left text-sm font-semibold break-words focus-visible:ring-2 focus-visible:outline-none disabled:cursor-default"
              >
                {group.name}
              </button>
            )}
            {editing ? null : (
              <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
                {count} {plural(noun, count)}
              </span>
            )}
          </div>
          {presentation.renderGroupCells?.(group, all)}
          <div className={ROW_CELL.menu}>
            <GroupMenu
              name={group.name}
              collapsed={group.collapsed}
              onStartRename={() => setEditing(true)}
              {...handlers}
            />
          </div>
        </div>
      </div>

      {/* The rows' sortable context stays mounted while collapsed so a
          row dropped on the folder row still has a target; only the
          rows hide. */}
      <ul id={panelId} hidden={!open} role="list">
        {subfolders.map((child) => (
          <li key={child.id} className={ROW_LINE}>
            <FolderSection
              node={child}
              board={board}
              forceOpen={forceOpen}
              noun={noun}
              dragActive={dragActive}
              heldFolder={heldFolder}
              dropTarget={dropTarget}
              folderDrop={folderDrop}
              suppressClickUntil={suppressClickUntil}
              presentation={presentation}
              density={density}
              sortNote={sortNote}
              groupHandlers={groupHandlers}
            />
          </li>
        ))}
        <SortableContext
          items={lists.map((c) => viewDndId(c.id))}
          strategy={verticalListSortingStrategy}
        >
          {lists.map((child) => (
            <BoardRowItem
              key={child.id}
              view={child.view}
              container={child.container}
              depth={child.depth}
              density={density}
              suppressClickUntil={suppressClickUntil}
              renderRow={presentation.renderRow}
              sortNote={sortNote}
            />
          ))}
        </SortableContext>
        {node.children.length === 0 ? (
          <li
            className={cn(
              ROW_LINE,
              "text-muted-foreground py-3 pr-3 text-sm transition-colors",
              "pl-[calc(3rem+var(--row-depth,0)*0.5rem)] @min-[30rem]/board:pl-[calc(5rem+var(--row-depth,0)*0.75rem)] @min-[42rem]/board:pl-[calc(5rem+var(--row-depth,0)*1.25rem)]",
              dragActive && "text-foreground/80",
            )}
            style={{ "--row-depth": node.depth } as React.CSSProperties}
          >
            {dragActive && heldFolder === null
              ? `Drop ${plural(noun, 2)} here`
              : forceOpen && count > 0
                ? `No ${plural(noun, 2)} here match your search.`
                : `Empty group — drag ${plural(noun, 2)} here or use a ${noun}'s menu.`}
          </li>
        ) : null}
      </ul>
    </section>
  );
}

/** The root rows: views in no group. Always a drop target, so a row can
 *  be dragged out of a group: while a row is held and groups exist, a
 *  "Drop here to ungroup" line shows under the table, search or not. */
function UngroupedRows({
  rows,
  hasGroups,
  noun,
  nothingVisible,
  dragActive,
  isDropTarget,
  heldFolder,
  suppressClickUntil,
  presentation,
  density,
  sortNote,
}: {
  rows: Array<Extract<BoardRow, { type: "view" }>>;
  hasGroups: boolean;
  noun: string;
  nothingVisible: boolean;
  dragActive: boolean;
  isDropTarget: boolean;
  /** A folder is held (its drop line reads "top level", not "ungroup"). */
  heldFolder: boolean;
  suppressClickUntil: React.MutableRefObject<number>;
  presentation: BoardRowsPresentation;
  density: BoardDensity;
  sortNote: string | null;
}) {
  const { setNodeRef } = useDroppable({ id: UNGROUPED_ID });
  const dropZone = dragActive && hasGroups;
  const message =
    nothingVisible && !dropZone
      ? `No ${plural(noun, 2)} match your search.`
      : null;
  return (
    <div
      ref={setNodeRef}
      className={cn(
        "rounded-b-lg transition-colors",
        isDropTarget && "bg-primary/5 ring-primary/60 ring-2 ring-inset",
      )}
    >
      <SortableContext
        items={rows.map((r) => viewDndId(r.id))}
        strategy={verticalListSortingStrategy}
      >
        {rows.length > 0 ? (
          <ul role="list" aria-label={hasGroups ? "Ungrouped" : undefined}>
            {rows.map((r) => (
              <BoardRowItem
                key={r.id}
                view={r.view}
                container={r.container}
                depth={r.depth}
                density={density}
                suppressClickUntil={suppressClickUntil}
                renderRow={presentation.renderRow}
                sortNote={sortNote}
              />
            ))}
          </ul>
        ) : null}
      </SortableContext>
      {dropZone ? (
        <p
          className={cn(
            ROW_LINE,
            "text-muted-foreground border-primary/40 m-2 rounded-md border border-dashed px-3 py-3 text-center text-sm",
          )}
        >
          {heldFolder ? "Drop here for the top level" : "Drop here to ungroup"}
        </p>
      ) : message ? (
        <p className={cn(ROW_LINE, "text-muted-foreground px-3 py-3 text-sm")}>
          {message}
        </p>
      ) : null}
    </div>
  );
}

const GRIP_CLASS = cn(
  "text-muted-foreground hover:text-foreground focus-visible:ring-ring inline-flex h-7 w-5 cursor-grab touch-none items-center justify-center rounded opacity-0 transition-opacity group-hover/row:opacity-100 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:outline-none active:cursor-grabbing",
  // Always visible on touch: there is no hover to reveal it.
  "[@media(pointer:coarse)]:opacity-100",
);

/** One sortable row. Like a card, the whole row is the pointer drag
 *  surface (press and hold on touch) apart from its buttons; the grip
 *  carries the keyboard path and the sortable ARIA attributes. */
function BoardRowItem({
  view,
  container,
  depth,
  density,
  suppressClickUntil,
  renderRow,
  sortNote,
}: {
  view: SavedViewView;
  container: string;
  depth: number;
  density: BoardDensity;
  suppressClickUntil: React.MutableRefObject<number>;
  renderRow: BoardRowsPresentation["renderRow"];
  sortNote: string | null;
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
  const { onKeyDown, surface } = dragSurface(listeners, suppressClickUntil);
  return (
    <li
      ref={setNodeRef}
      style={
        {
          transform: CSS.Translate.toString(transform),
          transition,
          "--row-depth": depth,
        } as React.CSSProperties
      }
      data-board-card=""
      data-board-row=""
      data-density={density}
      className={cn(
        ROW_GRID,
        ROW_PAD,
        ROW_LINE,
        "group/row hover:bg-muted/40 relative transition-colors",
        // The row's slot while it's held: an empty line the others
        // spring around.
        isDragging && "[&>*]:invisible",
      )}
    >
      <div role="presentation" className="contents" {...surface}>
        <button
          type="button"
          ref={setActivatorNodeRef}
          {...attributes}
          onKeyDown={onKeyDown}
          data-drag-handle=""
          aria-label={`Drag ${view.name}`}
          title={sortNote ?? undefined}
          className={cn(GRIP_CLASS, ROW_CELL.grip)}
        >
          <GripVertical className="size-4" aria-hidden="true" />
        </button>
        {renderRow(view, { depth, density, overlay: false })}
      </div>
    </li>
  );
}

// ──────────────────── new group dialog ────────────────────

function NewGroupDialog({
  open,
  onOpenChange,
  seedViewName,
  parentPath,
  pending,
  onSubmit,
  noun,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  seedViewName?: string;
  /** Set when the group is made inside another: that group's path. */
  parentPath?: string;
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
          parentPath={parentPath}
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
  parentPath,
  pending,
  onSubmit,
  onCancel,
  noun,
}: {
  seedViewName?: string;
  parentPath?: string;
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
            : parentPath
              ? `A group inside “${parentPath}”. Drag ${plural(noun, 2)} in, or use a ${noun}'s menu.`
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
