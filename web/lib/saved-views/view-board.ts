/**
 * Pure layout logic for the grouped Views index (Settings → Views).
 *
 * The server stores groups + ordered member ids per tab scope
 * (`/me/view-groups`); the saved views themselves come from the
 * existing list endpoints. `buildViewBoard` overlays the two into the
 * shape the board renders — each group with its resolved views, plus the
 * implicit "ungrouped" remainder — and the `move*` helpers produce the
 * next board for a drag or a menu action without touching React. Kept
 * free of DOM / dnd-kit so `web/tests/library/view-board.test.ts` can
 * pin the invariants (one home per view, stale ids dropped, stable
 * ungrouped sort).
 */
import type {
  LayoutGroupReq,
  ReplaceLayoutReq,
  SavedViewView,
  ViewGroupListView,
  ViewGroupView,
} from "@/lib/api/types";

/** Deepest nesting the server accepts: a top-level group is level 1. */
export const MAX_GROUP_DEPTH = 4;

/** The sorts every board has. `custom` renders each section in its
 *  hand-arranged (server-pinned) order, with never-arranged views
 *  trailing by name; the other keys ignore the pinned order. */
export type BuiltinViewSort = "custom" | "name" | "updated" | "created";

/** A built-in key, or a board's own (`board:read`, `board:refreshed` on
 *  the reading lists) that comes with a comparator. The prefix keeps typo
 *  checking on the built-in keys; a board key without a comparator sorts
 *  by name. */
export type ViewSort = BuiltinViewSort | `board:${string}`;

/** Order two views for a board-specific sort key. */
export type ViewCompare = (a: SavedViewView, b: SavedViewView) => number;

export const VIEW_SORT_OPTIONS: ReadonlyArray<{
  value: BuiltinViewSort;
  label: string;
}> = [
  { value: "custom", label: "Custom order" },
  { value: "name", label: "Name" },
  { value: "updated", label: "Recently updated" },
  { value: "created", label: "Recently added" },
];

export type BoardGroup = {
  id: string;
  name: string;
  collapsed: boolean;
  /** The enclosing group, `null` at the top level. */
  parentId: string | null;
  /** Set when the stored parent couldn't be placed (unknown to this
   *  cache, or on a cycle) and the group is shown at the top level
   *  instead: a display fallback that must never be saved. */
  displayOnlyParent?: true;
  views: SavedViewView[];
};

export type ViewBoard = {
  groups: BoardGroup[];
  /** Views in no group. Under the `custom` sort this is the server's
   *  pinned order followed by the un-pinned remainder by name; a reorder
   *  here is committed through the layout PUT like any group move. */
  ungrouped: SavedViewView[];
};

/** Droppable id of the implicit ungrouped section. Never collides with a
 *  group id (those are UUIDs). */
export const UNGROUPED_ID = "ungrouped";

export function sortViews(
  views: readonly SavedViewView[],
  sort: ViewSort,
  compare?: ViewCompare,
): SavedViewView[] {
  const out = views.slice();
  if (compare && sort.startsWith("board:")) {
    out.sort(compare);
    return out;
  }
  switch (sort) {
    case "updated":
      out.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      break;
    case "created":
      out.sort((a, b) => b.created_at.localeCompare(a.created_at));
      break;
    case "custom":
    case "name":
    default:
      out.sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
      );
  }
  return out;
}

/**
 * Order nodes into tree order: each node right before its subtree,
 * siblings by `position` (ties keep input order). A node whose parent is
 * unknown — or that sits on a parent cycle, which the server refuses but
 * a stale cache could still hold — is treated as top-level, so nothing is
 * ever dropped. Returns the nodes with their effective `parentId`.
 */
export function treeOrder<T extends { id: string; parentId: string | null }>(
  nodes: ReadonlyArray<T & { position?: number }>,
): T[] {
  const ids = new Set(nodes.map((n) => n.id));
  const parentOf = new Map(nodes.map((n) => [n.id, n.parentId]));
  // A parent chain that does not reach the top within `nodes.length`
  // steps is a cycle: those nodes are re-rooted.
  const rooted = (id: string): boolean => {
    let cur: string | null | undefined = parentOf.get(id);
    for (let i = 0; i <= nodes.length; i++) {
      if (cur === null || cur === undefined) return true;
      if (!ids.has(cur)) return true;
      cur = parentOf.get(cur);
    }
    return false;
  };
  const effective = nodes.map((n, i) => ({
    node: n,
    i,
    parentId:
      n.parentId !== null && ids.has(n.parentId) && rooted(n.id)
        ? n.parentId
        : null,
  }));
  const kids = new Map<string | null, typeof effective>();
  for (const e of effective) {
    const list = kids.get(e.parentId) ?? [];
    list.push(e);
    kids.set(e.parentId, list);
  }
  for (const list of kids.values()) {
    list.sort(
      (a, b) =>
        (a.node.position ?? a.i) - (b.node.position ?? b.i) || a.i - b.i,
    );
  }
  const out: T[] = [];
  const walk = (parent: string | null) => {
    for (const e of kids.get(parent) ?? []) {
      out.push(
        e.parentId === e.node.parentId
          ? e.node
          : ({ ...e.node, parentId: e.parentId } as T),
      );
      walk(e.node.id);
    }
  };
  walk(null);
  return out;
}

/**
 * Overlay `groups` onto `views`. A view id that appears in several groups
 * (shouldn't happen — the server enforces one home — but a stale cache
 * can race) is kept in the first group only; ids that no longer resolve
 * to a view are dropped silently. Anything left over is ungrouped: under
 * `custom` the ids in `ungroupedIds` lead in that order and the rest
 * follow by name, otherwise the whole section is sorted by `sort`.
 */
export function buildViewBoard(
  views: readonly SavedViewView[],
  groups: readonly ViewGroupView[],
  ungroupedIds: readonly string[] = [],
  sort: ViewSort = "custom",
  compare?: ViewCompare,
): ViewBoard {
  const byId = new Map(views.map((v) => [v.id, v]));
  const placed = new Set<string>();
  const boardGroups: BoardGroup[] = treeOrder(
    groups.map((g) => ({
      id: g.id,
      parentId: g.parent_id ?? null,
      position: g.position,
      group: g,
    })),
  ).map(({ group: g, parentId }) => {
    const members: SavedViewView[] = [];
    for (const id of g.view_ids) {
      const v = byId.get(id);
      if (!v || placed.has(id)) continue;
      placed.add(id);
      members.push(v);
    }
    return {
      id: g.id,
      name: g.name,
      collapsed: g.collapsed,
      parentId,
      ...(parentId !== (g.parent_id ?? null)
        ? { displayOnlyParent: true as const }
        : {}),
      views: members,
    };
  });
  const rest = views.filter((v) => !placed.has(v.id));
  if (sort !== "custom") {
    return { groups: boardGroups, ungrouped: sortViews(rest, sort, compare) };
  }
  const pinned: SavedViewView[] = [];
  const seen = new Set<string>();
  for (const id of ungroupedIds) {
    const v = byId.get(id);
    if (!v || placed.has(id) || seen.has(id)) continue;
    seen.add(id);
    pinned.push(v);
  }
  const trailing = sortViews(
    rest.filter((v) => !seen.has(v.id)),
    "name",
  );
  return { groups: boardGroups, ungrouped: [...pinned, ...trailing] };
}

/** Case-insensitive name/description filter. Empty query returns the
 *  board unchanged (same reference) so memoized consumers don't churn. */
export function filterBoard(board: ViewBoard, query: string): ViewBoard {
  const q = query.trim().toLowerCase();
  if (!q) return board;
  const match = (v: SavedViewView) =>
    v.name.toLowerCase().includes(q) ||
    (v.description ?? "").toLowerCase().includes(q);
  return {
    groups: board.groups.map((g) => ({ ...g, views: g.views.filter(match) })),
    ungrouped: board.ungrouped.filter(match),
  };
}

export type BoardLayout = { groups: LayoutGroupReq[]; ungrouped: string[] };

/** The wire payload for `PUT /me/view-groups/layout` (minus `scope`).
 *  Groups go in tree order with their parent, so each group's siblings
 *  appear in their order; the ungrouped order is sent in full, so
 *  whatever the user sees after a drop is exactly what the server pins. */
export function boardToLayout(board: ViewBoard): BoardLayout {
  return {
    groups: board.groups.map((g) => ({
      id: g.id,
      parent_id: g.parentId,
      view_ids: g.views.map((v) => v.id),
    })),
    ungrouped: board.ungrouped.map((v) => v.id),
  };
}

/** Which container currently holds `viewId`: a group id or
 *  `UNGROUPED_ID`; `null` when the board doesn't know the view. */
export function containerOf(board: ViewBoard, viewId: string): string | null {
  for (const g of board.groups) {
    if (g.views.some((v) => v.id === viewId)) return g.id;
  }
  return board.ungrouped.some((v) => v.id === viewId) ? UNGROUPED_ID : null;
}

/**
 * Move `viewId` into `toContainer` (a group id at any depth, or
 * `UNGROUPED_ID`) at `index` (end when omitted or out of range). Moving
 * within the same container reorders. Returns the same board reference
 * when nothing changes.
 */
export function moveView(
  board: ViewBoard,
  viewId: string,
  toContainer: string,
  index?: number,
): ViewBoard {
  const from = containerOf(board, viewId);
  if (from === null) return board;
  const view =
    from === UNGROUPED_ID
      ? board.ungrouped.find((v) => v.id === viewId)!
      : board.groups
          .find((g) => g.id === from)!
          .views.find((v) => v.id === viewId)!;

  const insertAt = (views: SavedViewView[], at: number | undefined) => {
    const i =
      at === undefined || at < 0 || at > views.length ? views.length : at;
    return [...views.slice(0, i), view, ...views.slice(i)];
  };

  if (toContainer === UNGROUPED_ID) {
    const without = board.ungrouped.filter((v) => v.id !== viewId);
    const ungrouped = insertAt(without, index);
    if (
      from === UNGROUPED_ID &&
      ungrouped.every((v, i) => v === board.ungrouped[i])
    ) {
      return board;
    }
    return {
      groups: board.groups.map((g) =>
        g.id === from
          ? { ...g, views: g.views.filter((v) => v.id !== viewId) }
          : g,
      ),
      ungrouped,
    };
  }
  if (!board.groups.some((g) => g.id === toContainer)) return board;

  const groups = board.groups.map((g) => {
    let views = g.views;
    if (g.id === from) views = views.filter((v) => v.id !== viewId);
    if (g.id === toContainer) views = insertAt(views, index);
    return views === g.views ? g : { ...g, views };
  });
  if (
    from === toContainer &&
    groups
      .find((g) => g.id === from)!
      .views.every(
        (v, i) => v === board.groups.find((g) => g.id === from)!.views[i],
      )
  ) {
    return board;
  }
  const ungrouped =
    from === UNGROUPED_ID
      ? board.ungrouped.filter((v) => v.id !== viewId)
      : board.ungrouped;
  return { groups, ungrouped };
}

// ──────────────────── the group tree ────────────────────

/** Re-establish tree order after a change that kept each group's
 *  relative order among its (possibly new) siblings. */
function retree(groups: readonly BoardGroup[]): BoardGroup[] {
  return treeOrder(groups.map((g, i) => ({ ...g, position: i }))).map(
    // Strip the helper's `position` again.
    ({ position: _p, ...g }) => g,
  );
}

/** The group with `id`, or `undefined`. */
export function findGroup(
  board: ViewBoard,
  id: string,
): BoardGroup | undefined {
  return board.groups.find((g) => g.id === id);
}

/** Level of a group: 1 at the top. 0 for an unknown id. */
export function groupDepth(board: ViewBoard, id: string): number {
  const parent = new Map(board.groups.map((g) => [g.id, g.parentId]));
  if (!parent.has(id)) return 0;
  let depth = 1;
  let cur = parent.get(id) ?? null;
  while (cur !== null && depth <= board.groups.length) {
    depth += 1;
    cur = parent.get(cur) ?? null;
  }
  return depth;
}

/** Child groups of `parentId` (`null` = the top level), in order. */
export function childGroups(
  board: ViewBoard,
  parentId: string | null,
): BoardGroup[] {
  return board.groups.filter((g) => g.parentId === parentId);
}

/** `id` and every group under it. */
export function subtreeIds(board: ViewBoard, id: string): Set<string> {
  const out = new Set<string>([id]);
  // Tree order: a subtree follows its root, so one pass collects it.
  for (const g of board.groups) {
    if (g.parentId !== null && out.has(g.parentId)) out.add(g.id);
  }
  return out;
}

/** How many levels the subtree rooted at `id` spans (1 = no subgroups). */
export function subtreeHeight(board: ViewBoard, id: string): number {
  const ids = subtreeIds(board, id);
  const base = groupDepth(board, id);
  let deepest = base;
  for (const gid of ids) deepest = Math.max(deepest, groupDepth(board, gid));
  return deepest - base + 1;
}

/** Every view in the group `id` and the groups under it, in tree order
 *  (a folder's roll-up input). */
export function subtreeViews(board: ViewBoard, id: string): SavedViewView[] {
  const ids = subtreeIds(board, id);
  return board.groups.filter((g) => ids.has(g.id)).flatMap((g) => g.views);
}

/** The names from the top level down to `id`, e.g. `["Marvel", "Events"]`. */
export function groupPath(board: ViewBoard, id: string): string[] {
  const byId = new Map(board.groups.map((g) => [g.id, g]));
  const out: string[] = [];
  let cur = byId.get(id);
  while (cur && out.length <= board.groups.length) {
    out.unshift(cur.name);
    cur = cur.parentId === null ? undefined : byId.get(cur.parentId);
  }
  return out;
}

/** Why a group cannot go under `parentId`, or `null` when it can. */
export type GroupMoveRefusal = "self" | "depth";

export function groupMoveRefusal(
  board: ViewBoard,
  id: string,
  parentId: string | null,
): GroupMoveRefusal | null {
  if (parentId === null) return null;
  if (subtreeIds(board, id).has(parentId)) return "self";
  if (groupDepth(board, parentId) + subtreeHeight(board, id) > MAX_GROUP_DEPTH)
    return "depth";
  return null;
}

/** What a held group is over (rows presentation): a folder row, the
 *  section under a folder (its rows), or the root. */
export type GroupDropTarget =
  | { kind: "row"; groupId: string }
  | { kind: "section"; groupId: string }
  | { kind: "root" };

/** Where a held group will land. */
export type GroupDropPlan = {
  parentId: string | null;
  /** Sibling index under `parentId` (end when undefined). */
  index: number | undefined;
  /** The folder row the line or ring shows on (`null`: the root). */
  target: string | null;
  zone: "before" | "after" | "into";
};

export type GroupDropState = {
  plan: GroupDropPlan | null;
  refusal: GroupMoveRefusal | null;
};

/** Fractions of a folder row's height: above `BEFORE` the held group
 *  goes before it, below `AFTER` after it, in between inside it. */
export const GROUP_DROP_EDGE = { BEFORE: 0.3, AFTER: 0.7 } as const;

/**
 * The one plan for a held group: what the drop line / ring, the label on
 * the lifted row, the announcement and the commit all read. Over a
 * folder row the pointer's height in that row (`pointerY` against the
 * row's current `rowRect`) picks before / into / after; over a section,
 * into it; over the root, the end of the top level. `pointerY` is
 * `null` without a pointer (keyboard): "into". A plan that would put
 * the group inside itself or past four levels comes back refused.
 */
export function planGroupDrop(
  board: ViewBoard,
  id: string,
  over: GroupDropTarget | null,
  pointerY: number | null,
  rowRect: { top: number; height: number } | null,
): GroupDropState {
  if (!over) return { plan: null, refusal: null };
  let plan: GroupDropPlan | null = null;
  if (over.kind === "root") {
    plan = { parentId: null, index: undefined, target: null, zone: "into" };
  } else if (subtreeIds(board, id).has(over.groupId)) {
    return { plan: null, refusal: "self" };
  } else if (over.kind === "section") {
    plan = {
      parentId: over.groupId,
      index: undefined,
      target: over.groupId,
      zone: "into",
    };
  } else {
    const target = findGroup(board, over.groupId);
    if (!target) return { plan: null, refusal: null };
    const rel =
      pointerY === null || !rowRect || rowRect.height <= 0
        ? 0.5
        : (pointerY - rowRect.top) / rowRect.height;
    const zone =
      rel < GROUP_DROP_EDGE.BEFORE
        ? "before"
        : rel > GROUP_DROP_EDGE.AFTER
          ? "after"
          : "into";
    if (zone === "into") {
      plan = {
        parentId: target.id,
        index: undefined,
        target: target.id,
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
        target: target.id,
        zone,
      };
    }
  }
  const refusal = groupMoveRefusal(board, id, plan.parentId);
  return refusal ? { plan: null, refusal } : { plan, refusal: null };
}

/** Can a new group be made inside `parentId`? */
export function canNestInside(board: ViewBoard, parentId: string): boolean {
  return groupDepth(board, parentId) < MAX_GROUP_DEPTH;
}

/**
 * Move group `id` (with its subtree) under `parentId` (`null` = top
 * level) at sibling `index` (end when omitted or out of range; counted
 * among the new siblings without the moved group). Returns the same
 * board when the move is refused (into itself, too deep) or changes
 * nothing.
 */
export function moveGroupTo(
  board: ViewBoard,
  id: string,
  parentId: string | null,
  index?: number,
): ViewBoard {
  const group = findGroup(board, id);
  if (!group) return board;
  if (parentId !== null && !findGroup(board, parentId)) return board;
  if (groupMoveRefusal(board, id, parentId) !== null) return board;
  const rest = board.groups.filter((g) => g.id !== id);
  const siblings = rest.filter((g) => g.parentId === parentId);
  const at =
    index === undefined || index < 0 || index > siblings.length
      ? siblings.length
      : index;
  // Placed by hand: whatever its stored parent was, this one is real.
  const { displayOnlyParent: _fallback, ...placed } = group;
  const moved = { ...placed, parentId };
  const before = siblings[at];
  const arranged = before
    ? rest.flatMap((g) => (g.id === before.id ? [moved, g] : [g]))
    : [...rest, moved];
  const groups = retree(arranged);
  const same =
    groups.length === board.groups.length &&
    groups.every(
      (g, i) =>
        g.id === board.groups[i]!.id &&
        g.parentId === board.groups[i]!.parentId,
    );
  return same ? board : { ...board, groups };
}

/** Move a group one step up (`-1`) or down (`1`) among its siblings. */
export function moveGroupAmongSiblings(
  board: ViewBoard,
  id: string,
  dir: -1 | 1,
): ViewBoard {
  const group = findGroup(board, id);
  if (!group) return board;
  const siblings = childGroups(board, group.parentId);
  const at = siblings.findIndex((g) => g.id === id);
  const to = at + dir;
  if (to < 0 || to >= siblings.length) return board;
  return moveGroupTo(board, id, group.parentId, to);
}

/** Reorder groups by moving the group at flat index `from` to the slot
 *  of the group at flat index `to`. Only siblings trade places (a drag
 *  among the flat cards); anything else returns the board unchanged. */
export function moveGroup(
  board: ViewBoard,
  from: number,
  to: number,
): ViewBoard {
  const a = board.groups[from];
  const b = board.groups[to];
  if (!a || !b || from === to || a.parentId !== b.parentId) return board;
  const siblings = childGroups(board, a.parentId);
  return moveGroupTo(
    board,
    a.id,
    a.parentId,
    siblings.findIndex((g) => g.id === b.id),
  );
}

/**
 * The board after deleting group `id` the way the server does: its child
 * groups take its place among its siblings, its views follow its
 * parent's own (or the ungrouped section's, at the top level).
 */
export function removeGroupLiftingContents(
  board: ViewBoard,
  id: string,
): ViewBoard {
  const group = findGroup(board, id);
  if (!group) return board;
  const parent = group.parentId;
  const arranged = board.groups.flatMap((g) => {
    if (g.id === id) return [];
    if (g.parentId === id) return [{ ...g, parentId: parent }];
    return [g];
  });
  // Children slot in where the deleted group sat: move them right
  // before its next sibling (or leave them at the end of the siblings).
  const siblings = childGroups(board, parent);
  const next = siblings[siblings.findIndex((g) => g.id === id) + 1];
  const kidIds = new Set(childGroups(board, id).map((k) => k.id));
  const kids = arranged.filter((g) => kidIds.has(g.id));
  const withoutKids = arranged.filter((g) => !kidIds.has(g.id));
  const placed = next
    ? withoutKids.flatMap((g) => (g.id === next.id ? [...kids, g] : [g]))
    : [...withoutKids, ...kids];
  const groups = retree(placed).map((g) =>
    parent !== null && g.id === parent
      ? { ...g, views: [...g.views, ...group.views] }
      : g,
  );
  return {
    groups,
    ungrouped:
      parent === null ? [...board.ungrouped, ...group.views] : board.ungrouped,
  };
}

/** Does the board show a group somewhere only as a display fallback? A
 *  layout built from it would save that fallback as the tree. */
export function hasDisplayOnlyParents(board: ViewBoard): boolean {
  return board.groups.some((g) => g.displayOnlyParent === true);
}

export function countViews(board: ViewBoard): number {
  return (
    board.ungrouped.length +
    board.groups.reduce((n, g) => n + g.views.length, 0)
  );
}

// ──────────────────── row model (tree presentation) ────────────────────

/**
 * One row of the board's tree presentation. A folder row carries its
 * `children` — its subfolders first, then its lists — a list row the
 * container it lives in. `depth` is the indent level, 0 at the root.
 */
export type BoardRow =
  | {
      type: "group";
      id: string;
      group: BoardGroup;
      depth: number;
      children: BoardRow[];
    }
  | {
      type: "view";
      id: string;
      view: SavedViewView;
      depth: number;
      /** A group id or `UNGROUPED_ID`. */
      container: string;
    };

/** The board as a tree: top-level folder rows in order, each with its
 *  subfolders and then its lists one level in, then the ungrouped lists
 *  at the root. */
export function boardRows(board: ViewBoard): BoardRow[] {
  const kids = new Map<string | null, BoardGroup[]>();
  for (const g of board.groups) {
    const list = kids.get(g.parentId) ?? [];
    list.push(g);
    kids.set(g.parentId, list);
  }
  const folder = (g: BoardGroup, depth: number): BoardRow => ({
    type: "group",
    id: g.id,
    group: g,
    depth,
    children: [
      ...(kids.get(g.id) ?? []).map((c) => folder(c, depth + 1)),
      ...g.views.map((v): BoardRow => ({
        type: "view",
        id: v.id,
        view: v,
        depth: depth + 1,
        container: g.id,
      })),
    ],
  });
  const groups = (kids.get(null) ?? []).map((g) => folder(g, 0));
  const root: BoardRow[] = board.ungrouped.map((v) => ({
    type: "view",
    id: v.id,
    view: v,
    depth: 0,
    container: UNGROUPED_ID,
  }));
  return [...groups, ...root];
}

/** Every view at or under `row`, in display order (a folder's roll-up
 *  input). */
export function rowViews(row: BoardRow): SavedViewView[] {
  if (row.type === "view") return [row.view];
  return row.children.flatMap(rowViews);
}

// ──────────────────── sorting a whole board ────────────────────

/** Order a board for display. `custom` returns it unchanged. Otherwise
 *  the ungrouped section is sorted, and with `groups` (the rows
 *  presentation) so are the members of every group at every depth.
 *  Groups themselves always keep their arranged order and place. */
export function sortBoard(
  board: ViewBoard,
  sort: ViewSort,
  compare?: ViewCompare,
  opts: { groups?: boolean } = {},
): ViewBoard {
  if (sort === "custom") return board;
  return {
    groups: opts.groups
      ? board.groups.map((g) => ({
          ...g,
          views: sortViews(g.views, sort, compare),
        }))
      : board.groups,
    ungrouped: sortViews(board.ungrouped, sort, compare),
  };
}

/**
 * The layout to commit while a sort is on. `next` is the displayed board
 * after a move (sorted, so its order inside a section means nothing);
 * `custom` is the stored hand-arranged board. Each section keeps its
 * stored order for the members it still has, and a member it gained is
 * appended at the end, so a sorted order is never pinned over the hand
 * arrangement. The tree — every group's parent and sibling order — and
 * membership come from `next` (groups are never sorted, so their order
 * there is the user's own). Pass `groups: false` when the group members
 * are displayed in their arranged order (the cards presentation): their
 * order in `next` is then the user's own and is sent as shown.
 */
export function layoutKeepingCustom(
  custom: ViewBoard,
  next: ViewBoard,
  opts: { groups?: boolean } = {},
): BoardLayout {
  const keep = (stored: readonly SavedViewView[], shown: SavedViewView[]) => {
    const members = new Set(shown.map((v) => v.id));
    const kept = stored.map((v) => v.id).filter((id) => members.has(id));
    const keptSet = new Set(kept);
    return [
      ...kept,
      ...shown.map((v) => v.id).filter((id) => !keptSet.has(id)),
    ];
  };
  const stored = new Map(custom.groups.map((g) => [g.id, g.views]));
  return {
    groups: next.groups.map((g) => ({
      id: g.id,
      parent_id: g.parentId,
      view_ids:
        opts.groups === false
          ? g.views.map((v) => v.id)
          : keep(stored.get(g.id) ?? [], g.views),
    })),
    ungrouped: keep(custom.ungrouped, next.ungrouped),
  };
}

/** Rebuild a board from a layout payload, taking the views and the group
 *  names / collapsed state from `base`. Ids `base` does not know are
 *  dropped. */
export function boardFromLayout(
  base: ViewBoard,
  layout: BoardLayout,
): ViewBoard {
  const byId = new Map<string, SavedViewView>();
  for (const g of base.groups) for (const v of g.views) byId.set(v.id, v);
  for (const v of base.ungrouped) byId.set(v.id, v);
  const resolve = (ids: readonly string[]) =>
    ids.flatMap((id) => {
      const v = byId.get(id);
      return v ? [v] : [];
    });
  const meta = new Map(base.groups.map((g) => [g.id, g]));
  return {
    groups: retree(
      layout.groups.flatMap((g) => {
        const m = meta.get(g.id);
        return m
          ? [
              {
                ...m,
                parentId: g.parent_id ?? null,
                views: resolve(g.view_ids),
              },
            ]
          : [];
      }),
    ),
    ungrouped: resolve(layout.ungrouped),
  };
}

// ──────────────────── what is sent ────────────────────

/**
 * Carry view ids the board does not know (`known`) into a layout, in
 * place. The server's last answer (`server`) can name views this page
 * has not loaded — one made or filed in another tab, or one deleted
 * elsewhere — and the layout PUT replaces the whole arrangement, so
 * leaving them out would move them out of their folder. Each unknown id
 * is put back in the same group (or the ungrouped order), right after
 * the known id that preceded it there, or first. The server drops any
 * that no longer exist. A group the layout no longer lists keeps nothing.
 */
export function carryUnknownIds(
  layout: BoardLayout,
  server: Pick<ViewGroupListView, "groups" | "ungrouped_ids"> | undefined,
  known: ReadonlySet<string>,
): BoardLayout {
  if (!server) return layout;
  const sent = new Set([
    ...layout.groups.flatMap((g) => g.view_ids),
    ...layout.ungrouped,
  ]);
  const carry = (raw: readonly string[], ids: readonly string[]) => {
    const out = ids.slice();
    let anchor: string | null = null;
    for (const id of raw) {
      if (known.has(id) || sent.has(id)) {
        if (out.includes(id)) anchor = id;
        continue;
      }
      const at = anchor === null ? 0 : out.indexOf(anchor) + 1;
      out.splice(at, 0, id);
      sent.add(id);
      anchor = id;
    }
    return out;
  };
  const rawOf = new Map(server.groups.map((g) => [g.id, g.view_ids]));
  return {
    groups: layout.groups.map((g) => {
      const raw = rawOf.get(g.id);
      return raw ? { ...g, view_ids: carry(raw, g.view_ids) } : g;
    }),
    ungrouped: carry(server.ungrouped_ids, layout.ungrouped),
  };
}

/** The server's list after deleting group `id` (contents up one level),
 *  for the optimistic cache. Mirrors `DELETE /me/view-groups/{id}`. */
export function listWithoutGroup(
  list: ViewGroupListView,
  id: string,
): ViewGroupListView {
  const gone = list.groups.find((g) => g.id === id);
  if (!gone) return list;
  const parent = gone.parent_id ?? null;
  const siblings = list.groups
    .filter((g) => (g.parent_id ?? null) === parent)
    .sort((a, b) => a.position - b.position);
  const children = list.groups
    .filter((g) => g.parent_id === id)
    .sort((a, b) => a.position - b.position);
  const order = siblings.flatMap((g) => (g.id === id ? children : [g]));
  const position = new Map(order.map((g, i) => [g.id, i]));
  return {
    ...list,
    groups: list.groups.flatMap((g) => {
      if (g.id === id) return [];
      const at = position.get(g.id);
      const moved =
        at === undefined ? g : { ...g, parent_id: parent, position: at };
      return moved.id === parent
        ? [{ ...moved, view_ids: [...moved.view_ids, ...gone.view_ids] }]
        : [moved];
    }),
    ungrouped_ids:
      parent === null
        ? [...list.ungrouped_ids, ...gone.view_ids]
        : list.ungrouped_ids,
  };
}

/** The parts of a stored group (`ViewGroupView`) the tree helpers read. */
type StoredGroup = Pick<
  ViewGroupView,
  "id" | "parent_id" | "position" | "view_ids"
>;

/**
 * The layout PUT that puts a deleted group back exactly where it was,
 * after the Undo re-created it as `newId`. `before` is the server's list
 * captured just before the delete (the stored arrangement, never a
 * sorted display); `current` is the list after the re-create.
 *
 * - The tree is `before`'s, with `oldId` swapped for `newId`: the group
 *   gets its parent and sibling slot back, and its former child groups
 *   move back under it. A group deleted in the meantime is skipped (its
 *   children hang from its nearest surviving ancestor); groups made in
 *   the meantime keep their current parent, after the restored ones.
 * - The restored group gets its members back; every other group keeps
 *   its current members, minus those (moves made in the meantime aren't
 *   clobbered).
 * - The ungrouped order is the current one minus anything grouped.
 */
export function restoredGroupLayout(
  scope: ReplaceLayoutReq["scope"],
  before: { groups: readonly StoredGroup[] },
  oldId: string,
  newId: string,
  current: { groups: readonly StoredGroup[]; ungrouped_ids: string[] },
): ReplaceLayoutReq {
  const swap = (id: string | null | undefined) =>
    id === oldId ? newId : (id ?? null);
  const now = new Map(current.groups.map((g) => [g.id, g]));
  const was = new Map(before.groups.map((g) => [swap(g.id)!, g]));
  const restored = before.groups.find((g) => g.id === oldId);
  const restoredMembers = new Set(restored?.view_ids ?? []);
  // Nearest ancestor (in `before`) that still exists.
  const survivingParent = (id: string): string | null => {
    let p = swap(was.get(id)?.parent_id);
    for (let i = 0; p !== null && i <= before.groups.length; i++) {
      if (now.has(p)) return p;
      p = swap(was.get(p)?.parent_id);
    }
    return null;
  };
  const ordered = treeOrder(
    before.groups.map((g) => ({
      id: swap(g.id)!,
      parentId: swap(g.parent_id),
      position: g.position,
    })),
  ).filter((g) => now.has(g.id));
  const groups: LayoutGroupReq[] = ordered.map((g) => ({
    id: g.id,
    parent_id: survivingParent(g.id),
    view_ids:
      g.id === newId
        ? (restored?.view_ids ?? [])
        : now.get(g.id)!.view_ids.filter((v) => !restoredMembers.has(v)),
  }));
  const listed = new Set(groups.map((g) => g.id));
  const listedParent = (p: string | null | undefined) =>
    p && (listed.has(p) || now.has(p)) ? p : null;
  for (const g of treeOrder(
    current.groups.map((c) => ({
      id: c.id,
      parentId: c.parent_id ?? null,
      position: c.position,
    })),
  )) {
    if (listed.has(g.id)) continue;
    groups.push({
      id: g.id,
      parent_id: listedParent(g.parentId),
      view_ids: now.get(g.id)!.view_ids.filter((v) => !restoredMembers.has(v)),
    });
    listed.add(g.id);
  }
  const grouped = new Set(groups.flatMap((g) => g.view_ids));
  return {
    scope,
    groups: capDepth(groups),
    ungrouped: current.ungrouped_ids.filter((id) => !grouped.has(id)),
  };
}

/**
 * Keep a restored tree within four levels. A group that would sit deeper
 * (one made inside a lifted group after the delete, say, which the
 * restore pushes back down a level) moves up to its deepest ancestor
 * that still allows it: it lands on level 4, after that ancestor's own
 * children. Order among the rest is unchanged.
 */
function capDepth(groups: LayoutGroupReq[]): LayoutGroupReq[] {
  const byId = new Map(groups.map((g) => [g.id, g]));
  const chain = (id: string | null | undefined): string[] => {
    const out: string[] = [];
    let cur = id ?? null;
    while (cur !== null && out.length <= groups.length) {
      out.unshift(cur);
      cur = byId.get(cur)?.parent_id ?? null;
    }
    return out;
  };
  // Parents first, so a moved parent's children are measured from its
  // new place.
  const ordered = treeOrder(
    groups.map((g, i) => ({
      id: g.id,
      parentId: g.parent_id ?? null,
      position: i,
    })),
  );
  const lifted: LayoutGroupReq[] = [];
  for (const { id } of ordered) {
    const g = byId.get(id)!;
    const ancestors = chain(g.parent_id); // top → parent
    if (ancestors.length + 1 > MAX_GROUP_DEPTH) {
      const parent = ancestors[MAX_GROUP_DEPTH - 2] ?? null;
      const moved = { ...g, parent_id: parent };
      byId.set(id, moved);
      lifted.push(moved);
    }
  }
  const liftedIds = new Set(lifted.map((g) => g.id));
  return [
    ...groups.filter((g) => !liftedIds.has(g.id)).map((g) => byId.get(g.id)!),
    ...lifted,
  ];
}
