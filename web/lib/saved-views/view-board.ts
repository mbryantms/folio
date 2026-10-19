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
  SavedViewView,
  ViewGroupView,
} from "@/lib/api/types";

/** `custom` renders the ungrouped section in its hand-arranged (server
 *  pinned) order, with never-arranged views trailing by name; the other
 *  keys ignore the pinned order and sort everything. */
export type ViewSort = "custom" | "name" | "updated" | "created";

export const VIEW_SORT_OPTIONS: ReadonlyArray<{
  value: ViewSort;
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
): SavedViewView[] {
  const out = views.slice();
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
): ViewBoard {
  const byId = new Map(views.map((v) => [v.id, v]));
  const placed = new Set<string>();
  const boardGroups: BoardGroup[] = groups
    .slice()
    .sort((a, b) => a.position - b.position)
    .map((g) => {
      const members: SavedViewView[] = [];
      for (const id of g.view_ids) {
        const v = byId.get(id);
        if (!v || placed.has(id)) continue;
        placed.add(id);
        members.push(v);
      }
      return { id: g.id, name: g.name, collapsed: g.collapsed, views: members };
    });
  const rest = views.filter((v) => !placed.has(v.id));
  if (sort !== "custom") {
    return { groups: boardGroups, ungrouped: sortViews(rest, sort) };
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
 *  The ungrouped order is sent in full, so whatever the user sees after
 *  a drop is exactly what the server pins. */
export function boardToLayout(board: ViewBoard): BoardLayout {
  return {
    groups: board.groups.map((g) => ({
      id: g.id,
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
 * Move `viewId` into `toContainer` (a group id or `UNGROUPED_ID`) at
 * `index` (end when omitted or out of range). Moving within the same
 * container reorders. Returns the same board reference when nothing
 * changes.
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
  const ungrouped =
    from === UNGROUPED_ID
      ? board.ungrouped.filter((v) => v.id !== viewId)
      : board.ungrouped;
  return { groups, ungrouped };
}

/** Reorder groups by moving the group at `from` to `to`. */
export function moveGroup(
  board: ViewBoard,
  from: number,
  to: number,
): ViewBoard {
  if (
    from === to ||
    from < 0 ||
    to < 0 ||
    from >= board.groups.length ||
    to >= board.groups.length
  ) {
    return board;
  }
  const groups = board.groups.slice();
  const [g] = groups.splice(from, 1);
  groups.splice(to, 0, g!);
  return { ...board, groups };
}

export function countViews(board: ViewBoard): number {
  return (
    board.ungrouped.length +
    board.groups.reduce((n, g) => n + g.views.length, 0)
  );
}
