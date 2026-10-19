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
  boardToLayout,
  buildViewBoard,
  containerOf,
  filterBoard,
  moveGroup,
  moveView,
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
