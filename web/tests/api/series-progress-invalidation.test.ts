/**
 * Series statistics refresh after a progress change.
 *
 * Regression: on a series page, marking an issue read (or unread) left
 * the "Read progress" tile stale until a reload — no progress write
 * invalidated the series detail query that carries `progress_summary`.
 * Every progress-changing mutation (and Undo's `restoreProgress`) now
 * invalidates the series detail, the series listings and the entity
 * series grids, and nothing deeper under the `series` prefix.
 */
import { QueryClient } from "@tanstack/react-query";
import type * as ReactQuery from "@tanstack/react-query";
import type * as MutationCore from "@/lib/api/mutations/_core";
import type { QueryKey } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const qc = new QueryClient();
type OnSuccess = (data: unknown, input: unknown, ctx: unknown) => unknown;
const captured: { onSuccess?: OnSuccess }[] = [];

vi.mock("@tanstack/react-query", async (orig) => ({
  ...(await orig<typeof ReactQuery>()),
  useQueryClient: () => qc,
}));
vi.mock("@/lib/api/mutations/_core", async (orig) => ({
  ...(await orig<typeof MutationCore>()),
  useApiMutation: (_build: unknown, opts: { onSuccess?: OnSuccess }) => {
    captured.push({ onSuccess: opts.onSuccess });
    return {};
  },
  apiMutate: vi.fn(async () => null),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));
vi.mock("@/lib/ui/undo-toast", () => ({ showUndoToast: vi.fn() }));

import {
  restoreProgress,
  useBulkMarkProgress,
  useBulkMarkSeriesMatchingProgress,
  useBulkMarkSeriesProgress,
  useUpsertIssueProgress,
  useUpsertSeriesProgress,
} from "@/lib/api/mutations";
import { invalidateSeriesProgress } from "@/lib/api/mutations/rails";
import { queryKeys } from "@/lib/api/queries";

const SERIES = queryKeys.series("saga");
const SERIES_BY_ID = queryKeys.series("0b6c1d1e-0000-0000-0000-000000000001");
const SERIES_LIST = queryKeys.seriesList({ sort: "name" } as never);
const ENTITY_SERIES = queryKeys.entityItems(
  "characters",
  "hazel",
  "series",
  24,
);
// Under the `series` prefix but carrying no progress — must stay fresh.
const SERIES_ISSUES = queryKeys.seriesIssues("saga", {} as never);
const SERIES_APPEARANCES = queryKeys.seriesAppearances("saga");
const ENTITY_ISSUES = queryKeys.entityItems(
  "characters",
  "hazel",
  "issues",
  24,
);

function seed() {
  qc.clear();
  for (const key of [
    SERIES,
    SERIES_BY_ID,
    SERIES_LIST,
    ENTITY_SERIES,
    SERIES_ISSUES,
    SERIES_APPEARANCES,
    ENTITY_ISSUES,
  ]) {
    qc.setQueryData(key as QueryKey, { seeded: true });
  }
}

function invalidated(key: readonly unknown[]): boolean {
  return qc.getQueryState(key as QueryKey)?.isInvalidated ?? false;
}

function expectSeriesProgressInvalidated() {
  expect(invalidated(SERIES)).toBe(true);
  expect(invalidated(SERIES_BY_ID)).toBe(true);
  expect(invalidated(SERIES_LIST)).toBe(true);
  expect(invalidated(ENTITY_SERIES)).toBe(true);
  expect(invalidated(SERIES_ISSUES)).toBe(false);
  expect(invalidated(SERIES_APPEARANCES)).toBe(false);
  expect(invalidated(ENTITY_ISSUES)).toBe(false);
}

/** Mount a hook (a plain call — the mocks make it hook-free) and return
 *  the `onSuccess` it handed `useApiMutation`. */
function onSuccessOf(mount: () => unknown): OnSuccess {
  captured.length = 0;
  mount();
  const cb = captured.at(-1)?.onSuccess;
  if (!cb) throw new Error("hook registered no onSuccess");
  return cb;
}

beforeEach(seed);

describe("progress mutations invalidate the series statistics", () => {
  it("useUpsertIssueProgress (single mark read / unread)", () => {
    const onSuccess = onSuccessOf(() => useUpsertIssueProgress());
    onSuccess(null, { issue_id: "i1", page: 0, finished: true }, undefined);
    expectSeriesProgressInvalidated();
  });

  it("useBulkMarkProgress (selection mark read / unread)", () => {
    const onSuccess = onSuccessOf(() => useBulkMarkProgress());
    onSuccess(
      { updated: 2, skipped: 0, forbidden: 0, not_found: 0 },
      { issue_ids: ["i1", "i2"], finished: false },
      undefined,
    );
    expectSeriesProgressInvalidated();
  });

  it("useBulkMarkSeriesMatchingProgress (select all matching)", () => {
    const onSuccess = onSuccessOf(() =>
      useBulkMarkSeriesMatchingProgress("saga"),
    );
    onSuccess(null, { finished: true }, undefined);
    expectSeriesProgressInvalidated();
  });

  it("useBulkMarkSeriesProgress (filter-view series selection)", () => {
    const onSuccess = onSuccessOf(() => useBulkMarkSeriesProgress());
    onSuccess(null, { series_ids: ["s1"], finished: true }, undefined);
    expectSeriesProgressInvalidated();
  });

  it("useUpsertSeriesProgress (mark all read / unread)", () => {
    const onSuccess = onSuccessOf(() => useUpsertSeriesProgress("saga"));
    onSuccess({ updated: 3 }, { finished: true }, undefined);
    expectSeriesProgressInvalidated();
  });

  it("restoreProgress (Undo of a mark)", async () => {
    await restoreProgress(qc, new Map([["i1", null]]));
    expectSeriesProgressInvalidated();
  });
});

describe("invalidateSeriesProgress", () => {
  it("marks without refetching when refetchActive is false", () => {
    const spy = vi.spyOn(qc, "invalidateQueries");
    invalidateSeriesProgress(qc, { refetchActive: false });
    expectSeriesProgressInvalidated();
    for (const [filters] of spy.mock.calls) {
      expect(filters?.refetchType).toBe("none");
    }
    spy.mockRestore();
  });
});
