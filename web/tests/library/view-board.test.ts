/**
 * `buildViewBoard` + the move helpers behind the grouped Views index.
 * Pins: one home per view, stale member ids dropped, ungrouped honours
 * the server-pinned order under `custom` and the chosen sort otherwise,
 * moves are atomic and produce the layout payload the server expects.
 */
import { describe, expect, it } from "vitest";

import type { SavedViewView, ViewGroupView } from "@/lib/api/types";
import {
  MAX_GROUP_DEPTH,
  UNGROUPED_ID,
  boardFromLayout,
  boardRows,
  boardToLayout,
  buildViewBoard,
  containerOf,
  filterBoard,
  moveGroup,
  layoutKeepingCustom,
  moveView,
  rowViews,
  sortBoard,
  sortViews,
  canNestInside,
  carryUnknownIds,
  groupDepth,
  groupMoveRefusal,
  groupPath,
  listWithoutGroup,
  moveGroupAmongSiblings,
  moveGroupTo,
  removeGroupLiftingContents,
  subtreeHeight,
  subtreeViews,
  treeOrder,
  GROUP_DROP_EDGE,
  hasDisplayOnlyParents,
  planGroupDrop,
  restoredGroupLayout,
} from "@/lib/saved-views/view-board";

function v(
  id: string,
  name = id,
  extra: Partial<SavedViewView> = {},
): SavedViewView {
  return {
    id,
    name,
    kind: "cbl",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...extra,
  } as unknown as SavedViewView;
}

function g(
  id: string,
  view_ids: string[],
  position = 0,
  name = id,
): ViewGroupView {
  return {
    id,
    name,
    scope: "cbl",
    position,
    collapsed: false,
    view_ids,
    created_at: "",
    updated_at: "",
  };
}

const views = [
  v("a", "Avengers"),
  v("b", "Batman"),
  v("c", "Civil War"),
  v("d", "Daredevil"),
];

describe("buildViewBoard", () => {
  it("places members in group order and the rest in ungrouped, sorted", () => {
    const board = buildViewBoard(
      [views[3]!, views[0]!, views[2]!, views[1]!],
      [g("g2", ["c"], 1), g("g1", ["b", "d"], 0)],
    );
    expect(board.groups.map((x) => x.id)).toEqual(["g1", "g2"]);
    expect(board.groups[0]!.views.map((x) => x.id)).toEqual(["b", "d"]);
    expect(board.groups[1]!.views.map((x) => x.id)).toEqual(["c"]);
    expect(board.ungrouped.map((x) => x.id)).toEqual(["a"]);
  });

  it("drops ids that no longer resolve and keeps a view in one group only", () => {
    const board = buildViewBoard(views, [
      g("g1", ["a", "zzz"]),
      g("g2", ["a", "b"], 1),
    ]);
    expect(board.groups[0]!.views.map((x) => x.id)).toEqual(["a"]);
    expect(board.groups[1]!.views.map((x) => x.id)).toEqual(["b"]);
    expect(board.ungrouped.map((x) => x.id)).toEqual(["c", "d"]);
  });

  it("under custom sort, pinned ungrouped ids lead and the rest trail by name", () => {
    const board = buildViewBoard(views, [g("g1", ["b"])], ["d", "zzz", "d"]);
    expect(board.ungrouped.map((x) => x.id)).toEqual(["d", "a", "c"]);
    // A pinned id that is actually grouped is ignored.
    expect(
      buildViewBoard(views, [g("g1", ["b"])], ["b", "c"]).ungrouped.map(
        (x) => x.id,
      ),
    ).toEqual(["c", "a", "d"]);
  });

  it("sorts ungrouped by the requested key, ignoring the pinned order", () => {
    const vs = [
      v("old", "Zed", { updated_at: "2025-01-01T00:00:00Z" }),
      v("new", "Alpha", { updated_at: "2026-06-01T00:00:00Z" }),
    ];
    expect(
      buildViewBoard(vs, [], ["old"], "name").ungrouped.map((x) => x.id),
    ).toEqual(["new", "old"]);
    expect(
      buildViewBoard(vs, [], ["old"], "updated").ungrouped.map((x) => x.id),
    ).toEqual(["new", "old"]);
    expect(
      buildViewBoard(
        [
          v("x", "X", { created_at: "2026-01-01T00:00:00Z" }),
          v("y", "Y", { created_at: "2026-02-01T00:00:00Z" }),
        ],
        [],
        [],
        "created",
      ).ungrouped.map((x) => x.id),
    ).toEqual(["y", "x"]);
  });
});

describe("moveView", () => {
  const board = buildViewBoard(views, [g("g1", ["a", "b"]), g("g2", ["c"], 1)]);

  it("moves across groups at an index and leaves one home", () => {
    const next = moveView(board, "a", "g2", 0);
    expect(next.groups[0]!.views.map((x) => x.id)).toEqual(["b"]);
    expect(next.groups[1]!.views.map((x) => x.id)).toEqual(["a", "c"]);
    expect(containerOf(next, "a")).toBe("g2");
    expect(boardToLayout(next)).toEqual({
      groups: [
        { id: "g1", parent_id: null, view_ids: ["b"] },
        { id: "g2", parent_id: null, view_ids: ["a", "c"] },
      ],
      ungrouped: ["d"],
    });
  });

  it("appends when the index is omitted or out of range", () => {
    expect(
      moveView(board, "d", "g1").groups[0]!.views.map((x) => x.id),
    ).toEqual(["a", "b", "d"]);
    expect(
      moveView(board, "d", "g1", 99).groups[0]!.views.map((x) => x.id),
    ).toEqual(["a", "b", "d"]);
  });

  it("reorders within a group", () => {
    const next = moveView(board, "b", "g1", 0);
    expect(next.groups[0]!.views.map((x) => x.id)).toEqual(["b", "a"]);
  });

  it("ungroups at an index, reorders inside ungrouped, and no-ops on a same spot", () => {
    const next = moveView(board, "c", UNGROUPED_ID, 0);
    expect(next.groups[1]!.views).toEqual([]);
    expect(next.ungrouped.map((x) => x.id)).toEqual(["c", "d"]);
    // Same position again → same reference.
    expect(moveView(next, "c", UNGROUPED_ID, 0)).toBe(next);
    // Reorder within ungrouped is a real change that reaches the payload.
    const swapped = moveView(next, "c", UNGROUPED_ID, 1);
    expect(swapped.ungrouped.map((x) => x.id)).toEqual(["d", "c"]);
    expect(boardToLayout(swapped).ungrouped).toEqual(["d", "c"]);
  });

  it("ignores unknown views and unknown targets", () => {
    expect(moveView(board, "nope", "g1")).toBe(board);
    expect(moveView(board, "a", "no-such-group")).toBe(board);
  });
});

describe("moveGroup / filterBoard", () => {
  const board = buildViewBoard(views, [
    g("g1", ["a"]),
    g("g2", ["b"], 1),
    g("g3", [], 2),
  ]);

  it("reorders groups and rejects out-of-range indices", () => {
    expect(moveGroup(board, 2, 0).groups.map((x) => x.id)).toEqual([
      "g3",
      "g1",
      "g2",
    ]);
    expect(moveGroup(board, 0, 5)).toBe(board);
    expect(moveGroup(board, 1, 1)).toBe(board);
  });

  it("filters by name across groups and ungrouped, keeping empty groups", () => {
    const next = filterBoard(board, "DARE");
    expect(next.groups.map((x) => x.views.length)).toEqual([0, 0, 0]);
    expect(next.ungrouped.map((x) => x.id)).toEqual(["d"]);
    expect(filterBoard(board, "  ")).toBe(board);
  });
});

describe("board-specific sorts", () => {
  const byLength = (a: SavedViewView, b: SavedViewView) =>
    b.name.length - a.name.length;

  it("sorts ungrouped with the caller's comparator for an extra key", () => {
    const board = buildViewBoard(
      views,
      [g("g1", ["a"])],
      [],
      "board:read",
      byLength,
    );
    // "Civil War" and "Daredevil" tie on length: the sort is stable.
    expect(board.ungrouped.map((x) => x.id)).toEqual(["c", "d", "b"]);
    // Groups keep their arranged order under any sort.
    expect(board.groups[0]!.views.map((x) => x.id)).toEqual(["a"]);
  });

  it("never lets a comparator override a built-in key, and falls back to name", () => {
    expect(sortViews(views, "name", byLength).map((x) => x.id)).toEqual([
      "a",
      "b",
      "c",
      "d",
    ]);
    expect(
      sortViews(views.slice().reverse(), "board:read").map((x) => x.id),
    ).toEqual(["a", "b", "c", "d"]);
  });
});

describe("boardRows (tree row model)", () => {
  it("lists folders in order with their views one level in, then the root", () => {
    const board = buildViewBoard(views, [
      g("g2", ["c"], 1),
      g("g1", ["b", "a"], 0),
    ]);
    const rows = boardRows(board);
    expect(
      rows.map((r) =>
        r.type === "group"
          ? `${r.id}@${r.depth}[${r.children.map((c) => `${c.id}@${c.depth}`).join(",")}]`
          : `${r.id}@${r.depth}`,
      ),
    ).toEqual(["g1@0[b@1,a@1]", "g2@0[c@1]", "d@0"]);
    const first = rows[0]!;
    expect(
      first.type === "group" && first.children[0]!.type === "view"
        ? first.children[0]!.container
        : null,
    ).toBe("g1");
    const root = rows[2]!;
    expect(root.type === "view" ? root.container : null).toBe(UNGROUPED_ID);
  });

  it("rowViews walks a folder to every view under it", () => {
    const rows = boardRows(buildViewBoard(views, [g("g1", ["b", "a"])]));
    expect(rowViews(rows[0]!).map((x) => x.id)).toEqual(["b", "a"]);
    expect(rowViews(rows[1]!).map((x) => x.id)).toEqual(["c"]);
  });

  it("follows a search-filtered board, keeping empty folders", () => {
    const board = buildViewBoard(views, [g("g1", ["b", "a"])]);
    const rows = boardRows(filterBoard(board, "dare"));
    expect(rows.map((r) => r.id)).toEqual(["g1", "d"]);
    expect(rows[0]!.type === "group" ? rows[0]!.children : null).toEqual([]);
  });
});

describe("sorted boards never pin a sorted order", () => {
  // Stored (custom) arrangement: g1 = [b, a], ungrouped = [d, c].
  const custom = buildViewBoard(views, [g("g1", ["b", "a"])], ["d", "c"]);
  const byName = (board: ReturnType<typeof buildViewBoard>) =>
    sortBoard(board, "name", undefined, { groups: true });

  it("sortBoard sorts inside groups only when asked; groups keep their order", () => {
    const two = buildViewBoard(
      views,
      [g("g2", ["d", "c"], 0), g("g1", ["b", "a"], 1)],
      [],
    );
    const rows = sortBoard(two, "name", undefined, { groups: true });
    expect(rows.groups.map((x) => x.id)).toEqual(["g2", "g1"]);
    expect(rows.groups[0]!.views.map((x) => x.id)).toEqual(["c", "d"]);
    const cards = sortBoard(two, "name");
    expect(cards.groups[0]!.views.map((x) => x.id)).toEqual(["d", "c"]);
    expect(sortBoard(two, "custom")).toBe(two);
  });

  it("rows: an unchanged section sends its stored order, not the sorted one", () => {
    const shown = byName(custom); // g1 shows [a, b], ungrouped [c, d]
    // A folder reorder or any commit with no membership change.
    expect(layoutKeepingCustom(custom, shown)).toEqual({
      groups: [{ id: "g1", parent_id: null, view_ids: ["b", "a"] }],
      ungrouped: ["d", "c"],
    });
  });

  it("rows: a move appends the list to the end of the target's stored order", () => {
    const shown = byName(custom);
    const moved = moveView(shown, "c", "g1", 0); // dropped at the top
    expect(layoutKeepingCustom(custom, moved)).toEqual({
      groups: [{ id: "g1", parent_id: null, view_ids: ["b", "a", "c"] }],
      ungrouped: ["d"],
    });
    // And the board shown after it is the stored order, sorted again.
    const next = boardFromLayout(moved, layoutKeepingCustom(custom, moved));
    expect(byName(next).groups[0]!.views.map((x) => x.id)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("cards: groups are sent as shown, the sorted ungrouped keeps its stored order", () => {
    const shown = sortBoard(custom, "name"); // ungrouped [c, d]
    const reordered = moveView(shown, "a", "g1", 0); // [a, b] by hand
    expect(layoutKeepingCustom(custom, reordered, { groups: false })).toEqual({
      groups: [{ id: "g1", parent_id: null, view_ids: ["a", "b"] }],
      ungrouped: ["d", "c"],
    });
    const ungroupedOne = moveView(shown, "a", UNGROUPED_ID);
    expect(
      layoutKeepingCustom(custom, ungroupedOne, { groups: false }),
    ).toEqual({
      groups: [{ id: "g1", parent_id: null, view_ids: ["b"] }],
      ungrouped: ["d", "c", "a"],
    });
  });

  it("boardFromLayout drops ids it does not know and keeps group names", () => {
    const b = boardFromLayout(custom, {
      groups: [
        { id: "g1", parent_id: null, view_ids: ["a", "zzz"] },
        { id: "gone", parent_id: null, view_ids: [] },
      ],
      ungrouped: ["b"],
    });
    expect(b.groups).toHaveLength(1);
    expect(b.groups[0]!.name).toBe("g1");
    expect(b.groups[0]!.views.map((x) => x.id)).toEqual(["a"]);
    expect(b.ungrouped.map((x) => x.id)).toEqual(["b"]);
  });
});

// ──────────────────── nested groups ────────────────────

function gp(
  id: string,
  view_ids: string[],
  position: number,
  parent_id: string | null,
): ViewGroupView {
  return { ...g(id, view_ids, position), parent_id };
}

const many = ["a", "b", "c", "d", "e", "f"].map((id) => v(id, id));

/** R ⊃ [S (b), T ⊃ [U (c)]] (a); Q (d); ungrouped e, f. */
const nested = () =>
  buildViewBoard(
    many,
    [
      gp("U", ["c"], 0, "T"),
      gp("Q", ["d"], 1, null),
      gp("T", [], 1, "R"),
      gp("R", ["a"], 0, null),
      gp("S", ["b"], 0, "R"),
    ],
    ["e", "f"],
  );
const ids = (b: { groups: { id: string }[] }) => b.groups.map((x) => x.id);
const parents = (b: { groups: { id: string; parentId: string | null }[] }) =>
  Object.fromEntries(b.groups.map((x) => [x.id, x.parentId]));

describe("group tree", () => {
  it("builds the board in tree order with each group's parent", () => {
    const b = nested();
    expect(ids(b)).toEqual(["R", "S", "T", "U", "Q"]);
    expect(parents(b)).toEqual({ R: null, S: "R", T: "R", U: "T", Q: null });
    expect(b.ungrouped.map((x) => x.id)).toEqual(["e", "f"]);
  });

  it("treats an unknown parent or a parent cycle as top-level, never dropping a group", () => {
    const b = buildViewBoard(many, [
      gp("A", [], 0, "gone"),
      gp("X", [], 1, "Y"),
      gp("Y", [], 2, "X"),
    ]);
    expect(ids(b).sort()).toEqual(["A", "X", "Y"]);
    expect(b.groups.every((x) => x.parentId === null)).toBe(true);
    expect(
      treeOrder([
        { id: "c", parentId: "p", position: 0 },
        { id: "p", parentId: null, position: 0 },
      ]).map((x) => x.id),
    ).toEqual(["p", "c"]);
  });

  it("measures depth, height, path, and the views under a folder", () => {
    const b = nested();
    expect(groupDepth(b, "R")).toBe(1);
    expect(groupDepth(b, "U")).toBe(3);
    expect(groupDepth(b, "nope")).toBe(0);
    expect(subtreeHeight(b, "R")).toBe(3);
    expect(subtreeHeight(b, "U")).toBe(1);
    expect(groupPath(b, "U")).toEqual(["R", "T", "U"]);
    expect(subtreeViews(b, "R").map((x) => x.id)).toEqual(["a", "b", "c"]);
    expect(canNestInside(b, "U")).toBe(true);
  });

  it("boardRows nests subfolders before lists, one level deeper each", () => {
    const rows = boardRows(nested());
    const show = (r: (typeof rows)[number]): string =>
      r.type === "group"
        ? `${r.id}@${r.depth}[${r.children.map(show).join(",")}]`
        : `${r.id}@${r.depth}`;
    expect(rows.map(show)).toEqual([
      "R@0[S@1[b@2],T@1[U@2[c@3]],a@1]",
      "Q@0[d@1]",
      "e@0",
      "f@0",
    ]);
    // A folder's roll-up covers every descendant.
    expect(rowViews(rows[0]!).map((x) => x.id)).toEqual(["b", "c", "a"]);
  });

  it("refuses moves into the group itself, its descendants, or past four levels", () => {
    const b = nested();
    expect(groupMoveRefusal(b, "R", "R")).toBe("self");
    expect(groupMoveRefusal(b, "R", "U")).toBe("self");
    expect(groupMoveRefusal(b, "Q", "U")).toBeNull(); // Q lands on level 4
    expect(groupMoveRefusal(b, "R", "Q")).toBeNull(); // 1 + 3 levels
    expect(groupMoveRefusal(b, "T", "S")).toBeNull(); // 2 + 2 levels
    expect(groupMoveRefusal(b, "T", null)).toBeNull();
    expect(moveGroupTo(b, "R", "U")).toBe(b);
    // A chain at the cap can't take a child.
    const deep = moveGroupTo(b, "Q", "U");
    expect(groupDepth(deep, "Q")).toBe(MAX_GROUP_DEPTH);
    expect(canNestInside(deep, "Q")).toBe(false);
    expect(groupMoveRefusal(deep, "S", "Q")).toBe("depth");
    expect(groupMoveRefusal(b, "T", "U")).toBe("self");
    expect(groupMoveRefusal(moveGroupTo(b, "Q", "S"), "T", "Q")).toBe(
      "depth", // 3 + 2 levels
    );
    expect(moveGroupTo(deep, "S", "Q")).toBe(deep);
  });

  it("moves a folder with its subtree into a folder, between siblings, and to the top", () => {
    const b = nested();
    const intoQ = moveGroupTo(b, "T", "Q");
    expect(ids(intoQ)).toEqual(["R", "S", "Q", "T", "U"]);
    expect(parents(intoQ)).toMatchObject({ T: "Q", U: "T" });
    const first = moveGroupTo(b, "T", "R", 0);
    expect(ids(first)).toEqual(["R", "T", "U", "S", "Q"]);
    const top = moveGroupTo(b, "U", null, 1);
    expect(ids(top)).toEqual(["R", "S", "T", "U", "Q"]);
    expect(parents(top).U).toBeNull();
    expect(boardToLayout(top).groups.map((x) => [x.id, x.parent_id])).toEqual([
      ["R", null],
      ["S", "R"],
      ["T", "R"],
      ["U", null],
      ["Q", null],
    ]);
    // Up / down only among siblings.
    expect(ids(moveGroupAmongSiblings(b, "T", -1))).toEqual([
      "R",
      "T",
      "U",
      "S",
      "Q",
    ]);
    expect(moveGroupAmongSiblings(b, "S", -1)).toBe(b);
    expect(moveGroupAmongSiblings(b, "Q", 1)).toBe(b);
    // Flat-index reorder (cards) trades siblings only.
    expect(ids(moveGroup(b, 4, 0))).toEqual(["Q", "R", "S", "T", "U"]);
    expect(moveGroup(b, 1, 4)).toBe(b);
  });

  it("deleting a folder lifts its contents one level, in its place", () => {
    const b = nested();
    const noT = removeGroupLiftingContents(b, "T");
    expect(ids(noT)).toEqual(["R", "S", "U", "Q"]);
    expect(parents(noT).U).toBe("R");
    const noR = removeGroupLiftingContents(b, "R");
    expect(ids(noR)).toEqual(["S", "T", "U", "Q"]);
    expect(noR.ungrouped.map((x) => x.id)).toEqual(["e", "f", "a"]);
    const noS = removeGroupLiftingContents(b, "S");
    expect(
      noS.groups.find((x) => x.id === "R")!.views.map((x) => x.id),
    ).toEqual(["a", "b"]);
  });

  it("listWithoutGroup mirrors the server's delete on the cached list", () => {
    const list = {
      scope: "cbl",
      groups: [
        gp("R", ["a"], 0, null),
        gp("S", ["b"], 0, "R"),
        gp("T", ["x"], 1, "R"),
        gp("U", ["c"], 0, "T"),
        gp("V", [], 1, "T"),
        gp("W", [], 2, "R"),
      ],
      ungrouped_ids: ["e"],
    };
    const out = listWithoutGroup(list, "T");
    const by = Object.fromEntries(out.groups.map((x) => [x.id, x]));
    expect(by.T).toBeUndefined();
    expect([by.U!.parent_id, by.U!.position]).toEqual(["R", 1]);
    expect([by.V!.parent_id, by.V!.position]).toEqual(["R", 2]);
    expect(by.W!.position).toBe(3);
    expect(by.R!.view_ids).toEqual(["a", "x"]);
    const top = listWithoutGroup(list, "R");
    expect(top.ungrouped_ids).toEqual(["e", "a"]);
    expect(
      top.groups.filter((x) => x.parent_id === null).map((x) => x.id),
    ).toEqual(["S", "T", "W"]);
  });

  it("sorts lists inside folders at every depth; folders keep their place", () => {
    const b = buildViewBoard(many, [
      gp("R", ["b", "a"], 0, null),
      gp("S", ["d", "c"], 0, "R"),
    ]);
    const sorted = sortBoard(b, "name", undefined, { groups: true });
    expect(ids(sorted)).toEqual(["R", "S"]);
    expect(sorted.groups[1]!.views.map((x) => x.id)).toEqual(["c", "d"]);
    expect(parents(sorted)).toEqual({ R: null, S: "R" });
    // A save while sorted: stored orders, tree from the move.
    const moved = moveGroupTo(moveView(sorted, "e", "S", 0), "S", null);
    expect(layoutKeepingCustom(b, moved)).toEqual({
      groups: [
        { id: "R", parent_id: null, view_ids: ["b", "a"] },
        { id: "S", parent_id: null, view_ids: ["d", "c", "e"] },
      ],
      ungrouped: ["f"],
    });
  });

  it("boardFromLayout rebuilds the tree from a layout", () => {
    const b = nested();
    const back = boardFromLayout(b, boardToLayout(moveGroupTo(b, "Q", "S")));
    expect(ids(back)).toEqual(["R", "S", "Q", "T", "U"]);
    expect(parents(back).Q).toBe("S");
  });
});

describe("carryUnknownIds", () => {
  const known = new Set(["a", "b", "c"]);
  it("puts ids the page doesn't know back in place inside their group", () => {
    const layout = {
      groups: [{ id: "G", parent_id: null, view_ids: ["b", "a"] }],
      ungrouped: ["c"],
    };
    const server = {
      groups: [gp("G", ["x", "a", "y", "b"], 0, null)],
      ungrouped_ids: ["c", "z"],
    };
    expect(carryUnknownIds(layout, server, known)).toEqual({
      groups: [{ id: "G", parent_id: null, view_ids: ["x", "b", "a", "y"] }],
      ungrouped: ["c", "z"],
    });
  });

  it("never re-adds a known id the move took out, and skips groups not sent", () => {
    const layout = {
      groups: [{ id: "G", parent_id: null, view_ids: [] }],
      ungrouped: ["a"],
    };
    const server = {
      groups: [gp("G", ["a", "x"], 0, null), gp("Gone", ["q"], 1, null)],
      ungrouped_ids: [],
    };
    expect(carryUnknownIds(layout, server, known)).toEqual({
      groups: [{ id: "G", parent_id: null, view_ids: ["x"] }],
      ungrouped: ["a"],
    });
    expect(carryUnknownIds(layout, undefined, known)).toBe(layout);
  });
});

describe("planGroupDrop (one plan for line, label, announcement, drop)", () => {
  // R ⊃ [S, T ⊃ [U]]; Q. A 48px folder row at y = 100.
  const b = nested();
  const row = { top: 100, height: 48 };
  const at = (f: number) => 100 + 48 * f;
  const over = (groupId: string) => ({ kind: "row" as const, groupId });
  const zone = (f: number, target = "T", held = "Q") =>
    planGroupDrop(b, held, over(target), at(f), row).plan?.zone;

  it("reads the pointer against the row: top before, middle into, bottom after", () => {
    expect([0, 0.1, 0.29].map((f) => zone(f))).toEqual([
      "before",
      "before",
      "before",
    ]);
    expect([0.3, 0.4, 0.5, 0.6, 0.7].map((f) => zone(f))).toEqual([
      "into",
      "into",
      "into",
      "into",
      "into",
    ]);
    expect([0.71, 0.9, 1].map((f) => zone(f))).toEqual([
      "after",
      "after",
      "after",
    ]);
    expect(GROUP_DROP_EDGE).toEqual({ BEFORE: 0.3, AFTER: 0.7 });
  });

  it("states the parent and sibling slot each zone means", () => {
    expect(planGroupDrop(b, "Q", over("T"), at(0.5), row).plan).toEqual({
      parentId: "T",
      index: undefined,
      target: "T",
      zone: "into",
    });
    expect(planGroupDrop(b, "Q", over("T"), at(0.1), row).plan).toEqual({
      parentId: "R",
      index: 1,
      target: "T",
      zone: "before",
    });
    expect(planGroupDrop(b, "Q", over("S"), at(0.9), row).plan).toEqual({
      parentId: "R",
      index: 1,
      target: "S",
      zone: "after",
    });
    // A section means into; the root means the end of the top level;
    // no pointer (keyboard) means into the row's folder.
    expect(
      planGroupDrop(b, "Q", { kind: "section", groupId: "U" }, null, null).plan
        ?.parentId,
    ).toBe("U");
    expect(planGroupDrop(b, "U", { kind: "root" }, 5, row).plan).toEqual({
      parentId: null,
      index: undefined,
      target: null,
      zone: "into",
    });
    expect(planGroupDrop(b, "Q", over("T"), null, null).plan?.zone).toBe(
      "into",
    );
    expect(planGroupDrop(b, "Q", null, 5, row)).toEqual({
      plan: null,
      refusal: null,
    });
  });

  it("refuses its own subtree and a fifth level, with the reason", () => {
    expect(planGroupDrop(b, "R", over("U"), at(0.5), row)).toEqual({
      plan: null,
      refusal: "self",
    });
    expect(planGroupDrop(b, "T", over("T"), at(0.1), row).refusal).toBe("self");
    // R spans three levels: into Q makes four (fine), into S five.
    expect(planGroupDrop(b, "R", over("Q"), at(0.5), row).plan?.zone).toBe(
      "into",
    );
    const deep = moveGroupTo(b, "Q", "U"); // Q on level 4
    expect(planGroupDrop(deep, "S", over("Q"), at(0.5), row)).toEqual({
      plan: null,
      refusal: "depth",
    });
    // Beside Q (level 4) is fine for a leaf.
    expect(
      planGroupDrop(deep, "S", over("Q"), at(0.9), row).plan?.parentId,
    ).toBe("U");
  });
});

describe("display-only parents are never saved", () => {
  it("flags a group re-rooted for display, and a hand placement clears it", () => {
    const b = buildViewBoard(many, [
      gp("A", [], 0, "gone"),
      gp("B", [], 1, null),
    ]);
    expect(b.groups.find((x) => x.id === "A")!.displayOnlyParent).toBe(true);
    expect(hasDisplayOnlyParents(b)).toBe(true);
    // Any other change keeps the fallback in the board: refused.
    expect(hasDisplayOnlyParents(moveView(b, "a", "B"))).toBe(true);
    // Placing A by hand states a real parent.
    expect(hasDisplayOnlyParents(moveGroupTo(b, "A", "B"))).toBe(false);
    expect(hasDisplayOnlyParents(nested())).toBe(false);
  });
});

describe("restoredGroupLayout never exceeds four levels", () => {
  it("lifts a group made since to the deepest ancestor that allows it", () => {
    // Before: L1 ⊃ D ⊃ C ⊃ E. Delete D (C, E move up), then N is made
    // under E (level 4). Restoring D pushes E to level 4 and N to 5.
    const before = {
      groups: [
        gp("L1", [], 0, null),
        gp("D", [], 0, "L1"),
        gp("C", [], 0, "D"),
        gp("E", [], 0, "C"),
      ],
    };
    const current = {
      groups: [
        gp("L1", [], 0, null),
        gp("C", [], 0, "L1"),
        gp("E", [], 0, "C"),
        gp("N", ["v"], 0, "E"),
        gp("D2", [], 1, null),
      ],
      ungrouped_ids: [],
    };
    const out = restoredGroupLayout("cbl", before, "D", "D2", current);
    expect(out.groups.map((x) => [x.id, x.parent_id])).toEqual([
      ["L1", null],
      ["D2", "L1"],
      ["C", "D2"],
      ["E", "C"],
      ["N", "C"],
    ]);
    expect(out.groups.find((x) => x.id === "N")!.view_ids).toEqual(["v"]);
  });
});
