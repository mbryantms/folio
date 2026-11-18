// @vitest-environment jsdom
/**
 * `useAdminSelectionActions` — the Fetch metadata + Rescan pair every
 * multi-select surface spreads into its Editing group.
 */
import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const fetchMutate = vi.fn();
const rescanMutate = vi.fn();
let role = "admin";

vi.mock("@/lib/api/queries", () => ({
  useMe: () => ({ data: { role } }),
}));
vi.mock("@/lib/api/mutations", async (importOriginal) => {
  return {
    ...(await importOriginal<Record<string, unknown>>()),
    useBulkMetadataFetch: () => ({ mutate: fetchMutate, isPending: false }),
    useBulkRescan: () => ({ mutate: rescanMutate, isPending: false }),
  };
});

import { useAdminSelectionActions } from "@/components/library/useAdminSelectionActions";

beforeEach(() => {
  role = "admin";
  fetchMutate.mockReset();
  rescanMutate.mockReset();
});

function action(
  actions: { id: string; disabled?: boolean; onClick: () => void }[],
  id: string,
) {
  const found = actions.find((a) => a.id === id);
  if (!found) throw new Error(`no ${id} action`);
  return found;
}

describe("useAdminSelectionActions", () => {
  it("gives a reader nothing", () => {
    role = "user";
    const { result } = renderHook(() =>
      useAdminSelectionActions({ issueIds: ["i1"], seriesIds: [] }),
    );
    expect(result.current.actions).toEqual([]);
  });

  it("sends issues and series together and runs onDone on success", () => {
    const onDone = vi.fn();
    const { result } = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: ["i1"], seriesIds: ["s1"] },
        { onDone },
      ),
    );
    const body = { issue_ids: ["i1"], series_ids: ["s1"] };

    action(result.current.actions, "fetch-metadata").onClick();
    expect(fetchMutate).toHaveBeenCalledWith(body, expect.any(Object));
    const opts = fetchMutate.mock.calls[0]?.[1] as { onSuccess: () => void };
    opts.onSuccess();
    expect(onDone).toHaveBeenCalledTimes(1);

    action(result.current.actions, "rescan").onClick();
    expect(rescanMutate).toHaveBeenCalledWith(body, expect.any(Object));
  });

  it("disables both on an empty selection or when the caller says so", () => {
    const empty = renderHook(() =>
      useAdminSelectionActions({ issueIds: [], seriesIds: [] }),
    );
    expect(empty.result.current.actions.every((a) => a.disabled)).toBe(true);
    const held = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: ["i1"], seriesIds: [] },
        { disabled: true },
      ),
    );
    expect(held.result.current.actions.every((a) => a.disabled)).toBe(true);
  });

  it("disables only the action whose server cap the selection passes", () => {
    // 201 issues: over the fetch cap (200), under the rescan cap (500).
    const issueIds = Array.from({ length: 201 }, (_, i) => `i${i}`);
    const { result } = renderHook(() =>
      useAdminSelectionActions({ issueIds, seriesIds: [] }),
    );
    expect(action(result.current.actions, "fetch-metadata").disabled).toBe(
      true,
    );
    expect(action(result.current.actions, "rescan").disabled).toBe(false);
  });
});
