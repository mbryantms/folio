/**
 * `buildViewBoard` + the move helpers behind the grouped Views index.
 * Pins: one home per view, stale member ids dropped, ungrouped honours
 * the server-pinned order under `custom` and the chosen sort otherwise,
 * moves are atomic and produce the layout payload the server expects.
 */
import { describe, expect, it } from "vitest";

import type { SavedViewView, ViewGroupView } from "@/lib/api/types";
import {
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
        { id: "g1", view_ids: ["b"] },
        { id: "g2", view_ids: ["a", "c"] },
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
      groups: [{ id: "g1", view_ids: ["b", "a"] }],
      ungrouped: ["d", "c"],
    });
  });

  it("rows: a move appends the list to the end of the target's stored order", () => {
    const shown = byName(custom);
    const moved = moveView(shown, "c", "g1", 0); // dropped at the top
    expect(layoutKeepingCustom(custom, moved)).toEqual({
      groups: [{ id: "g1", view_ids: ["b", "a", "c"] }],
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
      groups: [{ id: "g1", view_ids: ["a", "b"] }],
      ungrouped: ["d", "c"],
    });
    const ungroupedOne = moveView(shown, "a", UNGROUPED_ID);
    expect(
      layoutKeepingCustom(custom, ungroupedOne, { groups: false }),
    ).toEqual({
      groups: [{ id: "g1", view_ids: ["b"] }],
      ungrouped: ["d", "c", "a"],
    });
  });

  it("boardFromLayout drops ids it does not know and keeps group names", () => {
    const b = boardFromLayout(custom, {
      groups: [
        { id: "g1", view_ids: ["a", "zzz"] },
        { id: "gone", view_ids: [] },
      ],
      ungrouped: ["b"],
    });
    expect(b.groups).toHaveLength(1);
    expect(b.groups[0]!.name).toBe("g1");
    expect(b.groups[0]!.views.map((x) => x.id)).toEqual(["a"]);
    expect(b.ungrouped.map((x) => x.id)).toEqual(["b"]);
  });
});
