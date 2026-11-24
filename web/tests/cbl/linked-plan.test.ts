/**
 * Linked reading lists (PR 2b): the list page's chapter layout rules.
 * Finished chapters fold, the entries query starts at the first unfolded
 * chapter, and an event band shows only once its chapter is fully loaded.
 */
import { describe, expect, it } from "vitest";

import { cblEntriesPageQuery } from "@/lib/api/queries";
import type {
  ReadingListChapterView,
  ReadingListLinkView,
} from "@/lib/api/types";
import {
  buildChapterBlocks,
  foldedChapters,
  seekStart,
  type PlanItem,
} from "@/lib/cbl/linked-plan";
import { eventBandState } from "@/components/cbl/links/EventBand";
import { linkableLists } from "@/components/cbl/links/LinkDialogs";

function chapter(
  index: number,
  start: number,
  end: number,
  read: number,
  inLib: number,
  events: string[] = [],
): ReadingListChapterView {
  return {
    index,
    start_position: start,
    end_position: end,
    label: `C${index}`,
    event_link_ids: events,
    total: end - start + 1,
    in_library: inLib,
    read,
  };
}

function link(
  id: string,
  over: Partial<ReadingListLinkView> = {},
): ReadingListLinkView {
  return {
    id,
    kind: "event",
    placement: "manual",
    child_list_id: `list-${id}`,
    child_name: `Event ${id}`,
    anchor: { position: 4, status: "placed", series: "X", number: "1" },
    sort_within_anchor: 0,
    total: 4,
    in_library: 4,
    read: 0,
    first_unread: null,
    child_view_id: null,
    skipped: false,
    ...over,
  };
}

const item = (p: number): PlanItem<number> => ({
  kind: "entry",
  entry: p,
  position: p,
});

const chapters = [
  chapter(0, 0, 4, 5, 5, ["a"]),
  chapter(1, 5, 9, 2, 5, ["b"]),
  chapter(2, 10, 14, 0, 5),
];

describe("foldedChapters / seekStart", () => {
  it("folds finished chapters and starts the query at the first open one", () => {
    const folded = foldedChapters(chapters, {}, false);
    expect([...folded]).toEqual([0]);
    expect(seekStart(chapters, folded)).toBe(5);
  });

  it("show-read and a header click override the default", () => {
    expect(foldedChapters(chapters, {}, true).size).toBe(0);
    const f = foldedChapters(chapters, { 0: false, 2: true }, false);
    expect([...f].sort()).toEqual([2]);
    expect(seekStart(chapters, f)).toBeUndefined();
  });

  it("uses the chapters finished at load, not ones finished since", () => {
    const f = foldedChapters(chapters, {}, false, new Set());
    expect(f.size).toBe(0);
  });

  it("starts at the last chapter when every chapter is folded", () => {
    const all = new Set([0, 1, 2]);
    expect(seekStart(chapters, all)).toBe(10);
    expect(seekStart([], new Set())).toBeUndefined();
  });
});

describe("buildChapterBlocks", () => {
  const events = [
    link("a"),
    link("b", {
      anchor: { position: 9, status: "placed", series: "X", number: "2" },
    }),
  ];

  it("draws a folded chapter as header + its events, and holds a band until its chapter is loaded", () => {
    const loaded = [5, 6, 7].map(item);
    const blocks = buildChapterBlocks(
      loaded,
      chapters,
      events,
      new Set([0]),
      true,
      false,
    );
    expect(blocks.map((b) => [b.chapter?.index, b.folded])).toEqual([
      [0, true],
      [1, false],
    ]);
    expect(blocks[0]!.events.map((e) => e.id)).toEqual(["a"]);
    expect(blocks[1]!.items).toHaveLength(3);
    // Positions 8–9 are still to load, so the band waits.
    expect(blocks[1]!.events).toEqual([]);
  });

  it("shows the band once a later position is loaded", () => {
    const loaded = [5, 6, 7, 8, 9, 10].map(item);
    const blocks = buildChapterBlocks(
      loaded,
      chapters,
      events,
      new Set([0]),
      true,
      false,
    );
    expect(blocks[1]!.events.map((e) => e.id)).toEqual(["b"]);
    expect(blocks[2]!.items.map((i) => i.position)).toEqual([10]);
  });

  it("without folding (a search), skips chapters with no matches", () => {
    const blocks = buildChapterBlocks(
      [item(12)],
      chapters,
      events,
      new Set(),
      false,
      true,
    );
    expect(blocks.map((b) => b.chapter?.index)).toEqual([2]);
  });

  it("one plain block when the list has no chapters", () => {
    const blocks = buildChapterBlocks([item(0)], [], [], new Set(), true, true);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]!.chapter).toBeNull();
  });
});

describe("eventBandState", () => {
  it("full until finished; skipped wins", () => {
    expect(eventBandState(link("x"))).toBe("full");
    expect(eventBandState(link("x", { read: 4 }))).toBe("finished");
    expect(eventBandState(link("x", { read: 0, in_library: 0 }))).toBe("full");
    expect(eventBandState(link("x", { read: 4, skipped: true }))).toBe(
      "skipped",
    );
  });
});

describe("linkableLists", () => {
  const mk = (id: string, owner: string | null) =>
    ({ id, owner_user_id: owner }) as never;
  it("drops the list itself, linked lists, and (for a shared parent) private lists", () => {
    const all = [
      mk("p", null),
      mk("s", null),
      mk("mine", "u"),
      mk("done", null),
    ];
    expect(
      linkableLists(
        all,
        { id: "p", owner_user_id: null },
        new Set(["done"]),
      ).map((l) => l.id),
    ).toEqual(["s"]);
    expect(
      linkableLists(all, { id: "x", owner_user_id: "u" }, new Set()).map(
        (l) => l.id,
      ),
    ).toEqual(["p", "s", "mine", "done"]);
  });
});

describe("cblEntriesPageQuery", () => {
  it("maps a chapter jump to from_position and anything else to cursor", () => {
    expect(cblEntriesPageQuery("from:120")).toEqual({ from_position: 120 });
    expect(cblEntriesPageQuery("abc")).toEqual({ cursor: "abc" });
    expect(cblEntriesPageQuery(undefined)).toEqual({ cursor: undefined });
  });
});
