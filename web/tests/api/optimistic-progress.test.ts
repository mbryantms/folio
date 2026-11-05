/**
 * Optimistic mark read / unread: the caches flip before the request,
 * roll back on error, and Undo restores each issue's previous progress.
 */
import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type * as Queries from "@/lib/api/queries";

const m = vi.hoisted(() => ({ apiMutate: vi.fn(), jsonFetch: vi.fn() }));
vi.mock("@/lib/api/mutations/_core", () => ({ apiMutate: m.apiMutate }));
vi.mock("@/lib/api/queries", async (importOriginal) => ({
  ...(await importOriginal<typeof Queries>()),
  jsonFetch: m.jsonFetch,
}));

import {
  applyOptimisticProgress,
  capturePriorProgress,
  issueProgressKey,
  restoreProgress,
  rollbackProgress,
  type ProgressMap,
} from "@/lib/api/mutations/progress";
import { queryKeys } from "@/lib/api/queries";
import type { ContinueReadingView, ProgressView } from "@/lib/api/types";

const rec = (
  issue_id: string,
  page: number,
  finished: boolean,
  percent: number,
): ProgressView => ({
  issue_id,
  page,
  finished,
  percent,
  run: 0,
  updated_at: "2026-11-01T00:00:00Z",
});

let qc: QueryClient;
beforeEach(() => {
  qc = new QueryClient();
  m.apiMutate.mockReset().mockResolvedValue(null);
  m.jsonFetch.mockReset();
});

describe("optimistic progress", () => {
  it("flips the map, the Continue Reading card and the issue record, then rolls back", async () => {
    const map: ProgressMap = new Map([["a", rec("a", 4, false, 0.4)]]);
    qc.setQueryData(queryKeys.userProgress, map);
    const cr = {
      items: [
        {
          issue: { id: "a" },
          series_name: "Saga",
          progress: { last_page: 4, percent: 0.4, updated_at: "" },
        },
      ],
    } as unknown as ContinueReadingView;
    qc.setQueryData(queryKeys.continueReading, cr);

    const snap = await applyOptimisticProgress(qc, [
      { issue_id: "a", page: 9, finished: true },
      { issue_id: "b", page: 9, finished: true },
    ]);
    const next = qc.getQueryData<ProgressMap>(queryKeys.userProgress)!;
    expect(next.get("a")).toMatchObject({ finished: true, percent: 1 });
    expect(next.get("b")).toMatchObject({ finished: true, percent: 1 });
    expect(
      qc.getQueryData<ContinueReadingView>(queryKeys.continueReading)!.items[0]!
        .progress.percent,
    ).toBe(1);
    expect(qc.getQueryData(issueProgressKey("a"))).toMatchObject({
      finished: true,
    });

    rollbackProgress(qc, snap);
    expect(qc.getQueryData(queryKeys.userProgress)).toBe(map);
    expect(qc.getQueryData(queryKeys.continueReading)).toEqual(cr);
    expect(qc.getQueryData(issueProgressKey("a"))).toBeUndefined();
  });

  it("doesn't seed a progress map that was never loaded", async () => {
    await applyOptimisticProgress(qc, [
      { issue_id: "a", page: 0, finished: false },
    ]);
    expect(qc.getQueryData(queryKeys.userProgress)).toBeUndefined();
    expect(qc.getQueryData(issueProgressKey("a"))).toMatchObject({
      finished: false,
      percent: 0,
    });
  });

  it("captures prior state from the map, or asks for the one record", async () => {
    qc.setQueryData<ProgressMap>(
      queryKeys.userProgress,
      new Map([["a", rec("a", 3, false, 0.3)]]),
    );
    const prior = await capturePriorProgress(qc, ["a", "b"]);
    expect(prior.get("a")).toMatchObject({ page: 3 });
    expect(prior.get("b")).toBeNull();
    expect(m.jsonFetch).not.toHaveBeenCalled();

    const qc2 = new QueryClient();
    m.jsonFetch.mockResolvedValueOnce({ records: [rec("z", 9, true, 1)] });
    const one = await capturePriorProgress(qc2, ["z"]);
    expect(m.jsonFetch).toHaveBeenCalledWith("/progress?issue_id=z");
    expect(one.get("z")).toMatchObject({ finished: true });
  });

  it("Undo restores unread, mid-issue and finished issues", async () => {
    await restoreProgress(
      qc,
      new Map<string, ProgressView | null>([
        ["new", null],
        ["mid", rec("mid", 5, false, 0.5)],
        ["done", rec("done", 9, true, 1)],
      ]),
    );
    expect(m.apiMutate.mock.calls.map(([c]) => c)).toEqual([
      {
        path: "/me/progress/bulk",
        method: "POST",
        body: { issue_ids: ["new"], finished: false },
      },
      {
        path: "/me/progress/bulk",
        method: "POST",
        body: { issue_ids: ["done"], finished: true, backfill: true },
      },
      {
        path: "/progress",
        method: "POST",
        body: { issue_id: "mid", page: 5, finished: false },
      },
    ]);
    expect(qc.getQueryData(issueProgressKey("mid"))).toMatchObject({
      page: 5,
      finished: false,
    });
  });
});
