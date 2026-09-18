import { describe, expect, it, vi } from "vitest";
import { createProgressWriter } from "@/lib/reader/progress-writer";

describe("progress delivery", () => {
  it("retains HTTP and transport failures until a later flush", async () => {
    const send = vi
      .fn()
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValue(true);
    const writer = createProgressWriter(send);
    writer.set({ issue_id: "a", page: 4 });
    await writer.flush();
    await writer.flush();
    await writer.flush();
    await writer.flush();
    expect(send).toHaveBeenCalledTimes(3);
    expect(send).toHaveBeenLastCalledWith({ issue_id: "a", page: 4 });
  });
  it("serializes a newer page behind an in-flight write without losing it", async () => {
    let acknowledge!: (ok: boolean) => void;
    const send = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<boolean>((resolve) => {
            acknowledge = resolve;
          }),
      )
      .mockResolvedValue(true);
    const writer = createProgressWriter(send);
    writer.set({ issue_id: "a", page: 4 });
    const flush = writer.flush();
    writer.set({ issue_id: "a", page: 5 });
    expect(send).toHaveBeenCalledTimes(1);
    acknowledge(true);
    await flush;
    expect(send.mock.calls.map(([body]) => body.page)).toEqual([4, 5]);
    await writer.flush();
    expect(send).toHaveBeenCalledTimes(2);
  });
  it("does not replay pending writes after an account reset", async () => {
    const send = vi.fn().mockResolvedValue(false);
    const writer = createProgressWriter(send);
    writer.set({ issue_id: "a", page: 4 });
    await writer.flush();
    writer.clear();
    await writer.flush();
    expect(send).toHaveBeenCalledTimes(1);
  });
});

it("a failed issue does not block progress for another issue", async () => {
  const send = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  const writer = createProgressWriter(send);
  writer.set({ issue_id: "removed", page: 4 });
  writer.set({ issue_id: "current", page: 7 });
  await writer.flush();
  expect(send.mock.calls.map(([body]) => body.issue_id)).toEqual([
    "removed",
    "current",
  ]);
  send.mockResolvedValue(true);
  await writer.flush();
  expect(send).toHaveBeenLastCalledWith({ issue_id: "removed", page: 4 });
  expect(send).toHaveBeenCalledTimes(3);
});
