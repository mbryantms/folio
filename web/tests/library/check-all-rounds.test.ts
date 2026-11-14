/**
 * "Check for updates" carries on with the lists a `check-all` call did
 * not reach (`attempted: false`) until every list was checked, and merges
 * the rounds into one result in the lists' own order.
 */
import { describe, expect, it, vi } from "vitest";

import { ApiMutationError } from "@/lib/api/mutations/_core";
import type { CblBulkItemView, CblBulkResultView } from "@/lib/api/types";
import {
  GONE,
  MAX_IDS_PER_ROUND,
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

  it("never lets a round overwrite a row it did not ask about, and stops without progress", async () => {
    // A misbehaving server echoes an already-resolved list as attempted
    // (with different counts) while the lists asked about stay unreached.
    const echo = item("a", { changed: true, added: 9 });
    const { request, calls } = rounds(
      result([item("a"), missed("b"), missed("c")]),
      result([echo, missed("b"), missed("c")]),
      result([echo, missed("b"), missed("c")]),
    );
    const out = await checkAllUntilDone(request);
    expect(calls).toEqual([undefined, ["b", "c"]]);
    expect(out.items[0]).toEqual(item("a"));
    expect(out.changed_count).toBe(0);
    expect(out.failed_count).toBe(2);
  });

  it("sends at most MAX_IDS_PER_ROUND ids per round, the rest in later rounds", async () => {
    const n = MAX_IDS_PER_ROUND + 5;
    const all = Array.from({ length: n }, (_, i) => `l${i}`);
    const firstChunk = all.slice(1, MAX_IDS_PER_ROUND + 1);
    const rest = all.slice(MAX_IDS_PER_ROUND + 1);
    const { request, calls } = rounds(
      result([item(all[0]!), ...all.slice(1).map(missed)]),
      result(firstChunk.map((id) => item(id))),
      result(rest.map((id) => item(id))),
    );
    const out = await checkAllUntilDone(request);
    expect(calls).toEqual([undefined, firstChunk, rest]);
    expect(calls.every((c) => (c?.length ?? 0) <= MAX_IDS_PER_ROUND)).toBe(
      true,
    );
    expect(out.items.map((i) => i.id)).toEqual(all);
    expect(out.failed_count).toBe(0);
  });

  it("sends no further round once abandoned, and rejects with an AbortError", async () => {
    const controller = new AbortController();
    const calls: Array<string[] | undefined> = [];
    const request: CheckAllRequest = async (ids) => {
      calls.push(ids);
      // The page goes away while the first round is in flight.
      controller.abort();
      return result([item("a"), missed("b")]);
    };
    await expect(
      checkAllUntilDone(request, undefined, controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(calls).toEqual([undefined]);
  });

  it("resolves a list a round asked about but did not answer for, and never asks again", async () => {
    // `c` was deleted (or is no longer refreshable) between rounds.
    const { request, calls } = rounds(
      result([item("a"), missed("b"), missed("c")]),
      result([missed("b")]),
      result([item("b")]),
    );
    const out = await checkAllUntilDone(request);
    expect(calls).toEqual([undefined, ["b", "c"], ["b"]]);
    expect(out.items.map((i) => [i.id, i.ok, i.attempted, i.error])).toEqual([
      ["a", true, true, null],
      ["b", true, true, null],
      ["c", false, true, GONE],
    ]);
    expect(out.failed_count).toBe(1);
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
