// @vitest-environment jsdom
/**
 * `useWantToReadSelectionAction` — the multi-select "Add to Want to Read".
 */
import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { addMutate, toastError } = vi.hoisted(() => ({
  addMutate: vi.fn(),
  toastError: vi.fn(),
}));
let collections: { id: string; system_key: string | null }[] | undefined;
let boundId: string | undefined;

vi.mock("sonner", () => ({ toast: { error: toastError } }));
vi.mock("@/lib/api/queries", () => ({
  useCollections: () => ({ data: collections }),
}));
vi.mock("@/lib/api/mutations", async (importOriginal) => {
  return {
    ...(await importOriginal<Record<string, unknown>>()),
    useBulkAddToWantToRead: (id: string) => {
      boundId = id;
      return { mutate: addMutate, isPending: false };
    },
  };
});

import { useWantToReadSelectionAction } from "@/components/collections/useWantToReadSelectionAction";

const targets = [
  { entry_kind: "issue" as const, ref_id: "i1" },
  { entry_kind: "series" as const, ref_id: "s1" },
];

beforeEach(() => {
  addMutate.mockReset();
  toastError.mockReset();
  collections = [
    { id: "c-other", system_key: null },
    { id: "c-wtr", system_key: "want_to_read" },
  ];
  boundId = undefined;
});

describe("useWantToReadSelectionAction", () => {
  it("adds the whole selection to the caller's Want to Read", () => {
    const onDone = vi.fn();
    const { result } = renderHook(() =>
      useWantToReadSelectionAction(targets, { onDone }),
    );
    expect(boundId).toBe("c-wtr");
    expect(result.current.action.disabled).toBe(false);
    result.current.action.onClick();
    expect(addMutate).toHaveBeenCalledWith(
      { members: targets },
      expect.any(Object),
    );
    addMutate.mock.calls[0]?.[1].onSuccess();
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("says so instead of posting while Want to Read isn't loaded", () => {
    collections = undefined;
    const { result } = renderHook(() => useWantToReadSelectionAction(targets));
    result.current.action.onClick();
    expect(addMutate).not.toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledTimes(1);
  });

  it("is disabled for an empty or over-cap selection", () => {
    expect(
      renderHook(() => useWantToReadSelectionAction([])).result.current.action
        .disabled,
    ).toBe(true);
    const many = Array.from({ length: 501 }, (_, i) => ({
      entry_kind: "issue" as const,
      ref_id: `i${i}`,
    }));
    expect(
      renderHook(() => useWantToReadSelectionAction(many)).result.current.action
        .disabled,
    ).toBe(true);
  });
});
