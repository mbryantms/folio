// @vitest-environment jsdom
/**
 * `useAdminSelectionActions` — the Fetch metadata + Rescan pair every
 * multi-select surface spreads into its Editing group.
 */
import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const fetchMutate = vi.fn();
const rescanMutate = vi.fn();
const coverageMutate = vi.fn();
const syncMutate = vi.fn();
let role = "admin";

vi.mock("@/lib/api/queries", () => ({
  useMe: () => ({ data: { role } }),
}));
vi.mock("@/lib/api/mutations", async (importOriginal) => {
  return {
    ...(await importOriginal<Record<string, unknown>>()),
    useBulkMetadataFetch: () => ({ mutate: fetchMutate, isPending: false }),
    useBulkRescan: () => ({ mutate: rescanMutate, isPending: false }),
    useBulkAnalyzeCoverage: () => ({
      mutate: coverageMutate,
      isPending: false,
    }),
    useBulkSetMetadataSync: () => ({ mutate: syncMutate, isPending: false }),
  };
});

import { useAdminSelectionActions } from "@/components/library/useAdminSelectionActions";

beforeEach(() => {
  role = "admin";
  fetchMutate.mockReset();
  rescanMutate.mockReset();
  coverageMutate.mockReset();
  syncMutate.mockReset();
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
      useAdminSelectionActions(
        { issueIds: ["i1"], seriesIds: [] },
        { scope: "issues" },
      ),
    );
    expect(result.current.actions).toEqual([]);
  });

  it("sends issues and series together and runs onDone on success", () => {
    const onDone = vi.fn();
    const { result } = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: ["i1"], seriesIds: ["s1"] },
        { scope: "mixed", onDone },
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
      useAdminSelectionActions(
        { issueIds: [], seriesIds: [] },
        { scope: "mixed" },
      ),
    );
    expect(empty.result.current.actions.every((a) => a.disabled)).toBe(true);
    const held = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: ["i1"], seriesIds: [] },
        { scope: "issues", disabled: true },
      ),
    );
    expect(held.result.current.actions.every((a) => a.disabled)).toBe(true);
  });

  it("disables only the action whose server cap the selection passes", () => {
    // 201 issues: over the fetch cap (200), under the rescan cap (500).
    const issueIds = Array.from({ length: 201 }, (_, i) => `i${i}`);
    const { result } = renderHook(() =>
      useAdminSelectionActions(
        { issueIds, seriesIds: [] },
        { scope: "issues" },
      ),
    );
    expect(action(result.current.actions, "fetch-metadata").disabled).toBe(
      true,
    );
    expect(action(result.current.actions, "rescan").disabled).toBe(false);
  });

  it("shows each scope only the actions its targets can take", () => {
    const ids = (scope: "issues" | "series" | "mixed") =>
      renderHook(() =>
        useAdminSelectionActions(
          { issueIds: ["i1"], seriesIds: ["s1"] },
          { scope },
        ),
      ).result.current.actions.map((a) => a.id);
    expect(ids("issues")).toEqual([
      "edit-metadata",
      "fetch-metadata",
      "rescan",
    ]);
    expect(ids("series")).toEqual([
      "fetch-metadata",
      "rescan",
      "analyze-coverage",
      "pause-sync",
      "resume-sync",
    ]);
    expect(ids("mixed")).toEqual([
      "edit-metadata",
      "fetch-metadata",
      "rescan",
      "analyze-coverage",
      "pause-sync",
      "resume-sync",
    ]);
  });

  it("sends only series ids to the series actions", () => {
    const { result } = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: ["i1"], seriesIds: ["s1", "s2"] },
        { scope: "mixed" },
      ),
    );
    action(result.current.actions, "analyze-coverage").onClick();
    expect(coverageMutate).toHaveBeenCalledWith(
      { series_ids: ["s1", "s2"] },
      expect.any(Object),
    );
    action(result.current.actions, "pause-sync").onClick();
    expect(syncMutate).toHaveBeenLastCalledWith(
      { series_ids: ["s1", "s2"], paused: true },
      expect.any(Object),
    );
    action(result.current.actions, "resume-sync").onClick();
    expect(syncMutate).toHaveBeenLastCalledWith(
      { series_ids: ["s1", "s2"], paused: false },
      expect.any(Object),
    );
  });

  it("disables series actions without series and edit without issues", () => {
    const issuesOnly = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: ["i1"], seriesIds: [] },
        { scope: "mixed" },
      ),
    ).result.current.actions;
    expect(action(issuesOnly, "analyze-coverage").disabled).toBe(true);
    expect(action(issuesOnly, "pause-sync").disabled).toBe(true);
    expect(action(issuesOnly, "edit-metadata").disabled).toBe(false);
    const seriesOnly = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: [], seriesIds: ["s1"] },
        { scope: "mixed" },
      ),
    ).result.current.actions;
    expect(action(seriesOnly, "edit-metadata").disabled).toBe(true);
    // 51 series: over the coverage cap (50), under the sync cap (100).
    const many = Array.from({ length: 51 }, (_, i) => `s${i}`);
    const big = renderHook(() =>
      useAdminSelectionActions(
        { issueIds: [], seriesIds: many },
        { scope: "series" },
      ),
    ).result.current.actions;
    expect(action(big, "analyze-coverage").disabled).toBe(true);
    expect(action(big, "pause-sync").disabled).toBe(false);
  });
});
