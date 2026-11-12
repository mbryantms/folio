/**
 * A manual refresh re-reads the list's refresh state whatever its result.
 *
 * Every refresh records its outcome on the list row (attempt time, and on
 * failure the kind and a count), so after a failed refresh the list page's
 * notice must re-read the new count and attempt time, and after a
 * successful one the `/lists` row marker must go. `useRefreshCblList`
 * therefore invalidates the list detail, the `/me/cbl-lists` list and the
 * refresh log on settle, success or error, by registry keys.
 */
import { QueryClient } from "@tanstack/react-query";
import type * as ReactQuery from "@tanstack/react-query";
import type * as MutationCore from "@/lib/api/mutations/_core";
import type { QueryKey } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const qc = new QueryClient();
type Settled = (
  data: unknown,
  error: unknown,
  input: unknown,
  ctx: unknown,
) => unknown;
const captured: { onSettled?: Settled; onSuccess?: unknown }[] = [];

vi.mock("@tanstack/react-query", async (orig) => ({
  ...(await orig<typeof ReactQuery>()),
  useQueryClient: () => qc,
}));
vi.mock("@/lib/api/mutations/_core", async (orig) => ({
  ...(await orig<typeof MutationCore>()),
  useApiMutation: (
    _build: unknown,
    opts: { onSettled?: Settled; onSuccess?: unknown },
  ) => {
    captured.push({ onSettled: opts.onSettled, onSuccess: opts.onSuccess });
    return {};
  },
  apiMutate: vi.fn(async () => null),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));

import { useRefreshCblList } from "@/lib/api/mutations";
import { queryKeys } from "@/lib/api/queries";

const ID = "0b6c1d1e-0000-0000-0000-000000000001";
const OTHER = "0b6c1d1e-0000-0000-0000-000000000002";

function seed() {
  qc.clear();
  for (const key of [
    queryKeys.cblList(ID),
    queryKeys.cblLists,
    queryKeys.cblRefreshLog(ID),
    queryKeys.cblList(OTHER),
  ]) {
    qc.setQueryData(key as QueryKey, { seeded: true });
  }
}

function invalidated(key: readonly unknown[]): boolean {
  return qc.getQueryState(key as QueryKey)?.isInvalidated ?? false;
}

function onSettledOf(): Settled {
  captured.length = 0;
  useRefreshCblList(ID);
  const cb = captured.at(-1)?.onSettled;
  if (!cb) throw new Error("useRefreshCblList registered no onSettled");
  return cb;
}

beforeEach(seed);

describe("useRefreshCblList invalidates the refresh state on settle", () => {
  it.each([
    ["success", { upstream_changed: true }, null],
    ["error", undefined, new Error("HTTP: upstream returned status 503")],
  ])("after %s", (_label, data, error) => {
    onSettledOf()(data, error, {}, undefined);
    expect(invalidated(queryKeys.cblList(ID))).toBe(true);
    expect(invalidated(queryKeys.cblLists)).toBe(true);
    expect(invalidated(queryKeys.cblRefreshLog(ID))).toBe(true);
    // Another list's detail is left alone.
    expect(invalidated(queryKeys.cblList(OTHER))).toBe(false);
  });

  it("invalidates in onSettled, not in an onSuccess that skips errors", () => {
    onSettledOf();
    expect(captured.at(-1)?.onSuccess).toBeUndefined();
  });
});
