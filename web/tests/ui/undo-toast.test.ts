import { beforeEach, describe, expect, it, vi } from "vitest";

const { toastSuccess, toastError } = vi.hoisted(() => ({
  toastSuccess: vi.fn((..._args: unknown[]) => "toast-id"),
  toastError: vi.fn(),
}));
vi.mock("sonner", () => ({
  toast: { success: toastSuccess, error: toastError },
}));

import { UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
import { restoreOrder, showUndoToast } from "@/lib/ui/undo-toast";

type Opts = {
  id?: string;
  duration?: number;
  description?: string;
  action: { label: string; onClick: () => void };
};
const lastOpts = () => toastSuccess.mock.calls[0]?.[1] as Opts;
const flush = () => new Promise((r) => setTimeout(r, 0));

describe("showUndoToast", () => {
  beforeEach(() => {
    toastSuccess.mockClear();
    toastError.mockClear();
  });

  it("shows the message with an Undo action and the shared duration", () => {
    showUndoToast({ message: 'Deleted "X"', onUndo: () => {} });
    expect(toastSuccess.mock.calls[0]?.[0]).toBe('Deleted "X"');
    const opts = lastOpts();
    expect(opts.duration).toBe(UNDO_TOAST_DURATION_MS);
    expect(opts.action.label).toBe("Undo");
    expect(opts.id).toBeUndefined();
  });

  it("passes a toast id through so it can replace an existing toast", () => {
    showUndoToast({ message: "m", onUndo: () => {}, id: "bulk-remove-1" });
    expect(lastOpts().id).toBe("bulk-remove-1");
  });

  it("runs onUndo once and confirms with the restored message", async () => {
    const onUndo = vi.fn(async () => {});
    showUndoToast({ message: "m", onUndo, restoredMessage: "Back" });
    lastOpts().action.onClick();
    lastOpts().action.onClick();
    await flush();
    expect(onUndo).toHaveBeenCalledTimes(1);
    expect(toastSuccess).toHaveBeenLastCalledWith("Back");
  });

  it("reports a failed undo as an error toast", async () => {
    showUndoToast({
      message: "m",
      onUndo: () => Promise.reject(new Error("nope")),
      failedMessage: "Couldn't",
    });
    lastOpts().action.onClick();
    await flush();
    expect(toastError).toHaveBeenCalledWith("Couldn't");
  });
});

describe("restoreOrder", () => {
  it("puts a re-added item back in its old slot", () => {
    // "b" was removed and re-added (appended) as "b2".
    expect(restoreOrder(["a", "c", "b2"], ["a", "b2", "c"])).toEqual([
      "a",
      "b2",
      "c",
    ]);
  });

  it("keeps items added since after the known ones, in current order", () => {
    expect(restoreOrder(["n1", "a", "n2", "b"], ["b", "a"])).toEqual([
      "b",
      "a",
      "n1",
      "n2",
    ]);
  });

  it("orders by a key function", () => {
    const cur = [
      { id: 1, k: "y" },
      { id: 2, k: "x" },
      { id: 3, k: null },
    ];
    expect(restoreOrder(cur, ["x", "y"], (e) => e.k).map((e) => e.id)).toEqual([
      2, 1, 3,
    ]);
  });
});
