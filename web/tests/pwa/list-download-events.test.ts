/**
 * Offline list downloads include linked events where they are read:
 * `dueEvents` picks the events worth downloading, `takeDue` hands each
 * one out once, before the first issue past the one it follows.
 */
import { describe, expect, it } from "vitest";

import { dueEvents, takeDue, type DueEvent } from "@/lib/pwa/downloads";
import type { ReadingListLinkView } from "@/lib/api/types";

function ev(
  list: string,
  position: number | null,
  over: Partial<ReadingListLinkView> = {},
): ReadingListLinkView {
  return {
    id: `link-${list}`,
    kind: "event",
    child_list_id: list,
    child_name: list,
    skipped: false,
    read: 0,
    in_library: 5,
    total: 5,
    anchor: { position, status: "placed" },
    ...over,
  } as unknown as ReadingListLinkView;
}

describe("dueEvents", () => {
  it("keeps placed, unskipped events with something unread", () => {
    const got = dueEvents([
      ev("a", 4),
      ev("skipped", 5, { skipped: true }),
      ev("read", 6, { read: 5 }),
      ev("none-owned", 7, { in_library: 0 }),
      ev("lost", 8, {
        anchor: { position: 8, status: "lost" },
      } as Partial<ReadingListLinkView>),
      ev("b", 9),
    ]);
    expect(got).toEqual([
      { listId: "a", after: 4 },
      { listId: "b", after: 9 },
    ]);
  });
});

describe("takeDue", () => {
  it("hands out each event once, before the first issue past its anchor", () => {
    const events: DueEvent[] = [
      { listId: "behind", after: 1 },
      { listId: "a", after: 4 },
      { listId: "b", after: 4 },
      { listId: "c", after: 9 },
    ];
    // The reader last read position 3; the next issue is at 6.
    expect(takeDue(events, 3, 6).map((e) => e.listId)).toEqual(["a", "b"]);
    // Asked again for the same stretch: nothing (already taken).
    expect(takeDue(events, 3, 6)).toEqual([]);
    // The issue at 12: the event after 9 is due first.
    expect(takeDue(events, 3, 12).map((e) => e.listId)).toEqual(["c"]);
    // End of the list: nothing left; the one behind the reader never is.
    expect(takeDue(events, 3, Number.POSITIVE_INFINITY)).toEqual([]);
  });

  it("includes an event that follows the issue just read", () => {
    const events: DueEvent[] = [{ listId: "a", after: 3 }];
    expect(takeDue(events, 3, 4).map((e) => e.listId)).toEqual(["a"]);
  });
});
