/**
 * "Check for updates" carries on with the lists a `check-all` call did
 * not reach (`attempted: false`) until every list was checked, and merges
 * the rounds into one result in the lists' own order.
 */
import { describe, expect, it, vi } from "vitest";

import { ApiMutationError } from "@/lib/api/mutations/_core";
import type { CblBulkItemView, CblBulkResultView } from "@/lib/api/types";
import {
  checkAllUntilDone,
  type CheckAllProgress,
  type CheckAllRequest,
} from "@/lib/cbl/check-all-rounds";

const NOT_ATTEMPTED =
  "not attempted: the request ran out of time; run it again";

function item(
  id: string,
  over: Partial<CblBulkItemView> = {},
): CblBulkItemView {
  return {
    id,
    name: `List ${id}`,
    ok: true,
    error: null,
    applied: false,
    changed: false,
    upstream_changed: false,
    added: 0,
    removed: 0,
    reordered: 0,
    rematched: 0,
    attempted: true,
    ...over,
  };
}

const missed = (id: string) =>
  item(id, { ok: false, error: NOT_ATTEMPTED, attempted: false });

function result(items: CblBulkItemView[]): CblBulkResultView {
  return {
    items,
    changed_count: items.filter((i) => i.changed).length,
    failed_count: items.filter((i) => !i.ok).length,
  };
}

/** A request stub answering each round in turn, recording what it sent. */
function rounds(...answers: Array<CblBulkResultView | Error>) {
  const calls: Array<string[] | undefined> = [];
  const request: CheckAllRequest = vi.fn(async (ids?: string[]) => {
    calls.push(ids);
    const next = answers.shift();
    if (next === undefined) throw new Error("unexpected round");
    if (next instanceof Error) throw next;
    return next;
  });
  return { request, calls };
}

describe("checkAllUntilDone", () => {
  it("makes one call, with no body, when every list was attempted", async () => {
    const first = result([
      item("a", { changed: true }),
      item("b", { ok: false, error: "HTTP: 502", attempted: true }),
    ]);
    const { request, calls } = rounds(first);
    const progress = vi.fn();
    const out = await checkAllUntilDone(request, progress);
    expect(calls).toEqual([undefined]);
    expect(out).toEqual(first);
    expect(progress).not.toHaveBeenCalled();
  });

  it("asks again with exactly the lists not attempted, and merges in the original order", async () => {
    const { request, calls } = rounds(
      result([
        item("a"),
        missed("b"),
        item("c", { changed: true }),
        missed("d"),
      ]),
      result([item("b", { changed: true }), missed("d")]),
      result([item("d", { ok: false, error: "timed out fetching the list" })]),
    );
    const progress: CheckAllProgress[] = [];
    const out = await checkAllUntilDone(request, (p) => progress.push(p));
    expect(calls).toEqual([undefined, ["b", "d"], ["d"]]);
    expect(progress).toEqual([
      { checked: 2, total: 4 },
      { checked: 3, total: 4 },
    ]);
    expect(out.items.map((i) => i.id)).toEqual(["a", "b", "c", "d"]);
    expect(out.items.every((i) => i.attempted)).toBe(true);
    expect(out.changed_count).toBe(2);
    expect(out.failed_count).toBe(1);
    expect(out.items[3]!.error).toBe("timed out fetching the list");
  });

  it("stops after a round that checked nothing", async () => {
    const { request, calls } = rounds(
      result([item("a"), missed("b"), missed("c")]),
      result([missed("b"), missed("c")]),
    );
    const out = await checkAllUntilDone(request);
    expect(calls).toEqual([undefined, ["b", "c"]]);
    expect(out.items.map((i) => [i.id, i.ok, i.error])).toEqual([
      ["a", true, null],
      ["b", false, NOT_ATTEMPTED],
      ["c", false, NOT_ATTEMPTED],
    ]);
    expect(out.failed_count).toBe(2);
  });

  it("does not start a second round when the first checked nothing", async () => {
    const { request, calls } = rounds(result([missed("a"), missed("b")]));
    const out = await checkAllUntilDone(request);
    expect(calls).toEqual([undefined]);
    expect(out.failed_count).toBe(2);
  });

  it("keeps the lists already checked when a later round fails", async () => {
    const { request, calls } = rounds(
      result([missed("a"), item("b", { changed: true }), missed("c")]),
      new ApiMutationError("rate limited", 429),
    );
    const out = await checkAllUntilDone(request);
    expect(calls).toEqual([undefined, ["a", "c"]]);
    expect(out.items.map((i) => [i.id, i.ok, i.error])).toEqual([
      ["a", false, "rate limited"],
      ["b", true, null],
      ["c", false, "rate limited"],
    ]);
    expect(out.changed_count).toBe(1);
    expect(out.failed_count).toBe(2);
  });

  it("lets a failed first round fail the check", async () => {
    const { request } = rounds(new ApiMutationError("offline", "network"));
    await expect(checkAllUntilDone(request)).rejects.toThrow("offline");
  });

  it("ignores lists a later round returns that were not asked about", async () => {
    const { request } = rounds(
      result([item("a"), missed("b")]),
      result([item("b"), item("z")]),
    );
    const out = await checkAllUntilDone(request);
    expect(out.items.map((i) => i.id)).toEqual(["a", "b"]);
    expect(out.failed_count).toBe(0);
  });
});
