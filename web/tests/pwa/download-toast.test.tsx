/**
 * The offline download dialog's toast follows the batch it started:
 * progress by pages / issues / bytes, then "Ready offline" (or what
 * failed), and the bootstrap's per-issue toast stands down meanwhile.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const t = vi.hoisted(() => ({
  loading: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
}));
vi.mock("sonner", () => ({ toast: t }));

import {
  batchProgress,
  isTrackedDownload,
  newlyActive,
  trackDownloadToast,
} from "@/lib/pwa/download-toast";
import type { DownloadManager } from "@/lib/pwa/downloads";
import type { DownloadRecord } from "@/lib/pwa/offline-store";

const rec = (
  issueId: string,
  status: DownloadRecord["status"],
  donePages = 0,
  pageCount = 10,
  bytes = 0,
): DownloadRecord =>
  ({
    issueId,
    status,
    donePages,
    pageCount,
    pages: [],
    bytes,
  }) as unknown as DownloadRecord;

function fakeManager(initial: DownloadRecord[]) {
  let records = initial;
  const listeners = new Set<() => void>();
  return {
    manager: {
      snapshot: () => records,
      subscribe: (l: () => void) => {
        listeners.add(l);
        return () => listeners.delete(l);
      },
    } as unknown as DownloadManager,
    set(next: DownloadRecord[]) {
      records = next;
      for (const l of listeners) l();
    },
    listeners,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  for (const f of Object.values(t)) f.mockReset();
});
afterEach(() => vi.useRealTimers());

describe("download toast", () => {
  it("only follows the downloads the request added", () => {
    const before = [rec("a", "downloading"), rec("b", "complete")];
    const after = [
      rec("a", "downloading"),
      rec("b", "queued"),
      rec("c", "queued"),
      rec("d", "complete"),
    ];
    expect(newlyActive(before, after)).toEqual(["b", "c"]);
  });

  it("measures progress by pages when they're known, else by issues", () => {
    expect(
      batchProgress(
        [rec("a", "complete", 10), rec("b", "downloading", 5)],
        ["a", "b"],
      ).percent,
    ).toBe(75);
    expect(
      batchProgress(
        [rec("a", "complete", 4, 4), rec("b", "queued", 0, 0)],
        ["a", "b"],
      ).percent,
    ).toBe(50);
  });

  it("updates one toast until the batch is ready offline", () => {
    const f = fakeManager([rec("a", "queued"), rec("b", "queued")]);
    trackDownloadToast(f.manager, ["a", "b"], "Saga");
    expect(t.loading).toHaveBeenCalledTimes(1);
    expect(t.loading.mock.calls[0]?.[1]).toMatchObject({
      id: "offline-download",
    });
    expect(isTrackedDownload("a")).toBe(true);

    f.set([rec("a", "complete", 10, 10, 2048), rec("b", "downloading", 5)]);
    vi.advanceTimersByTime(300);
    expect(t.loading).toHaveBeenCalledTimes(2);

    f.set([
      rec("a", "complete", 10, 10, 2048),
      rec("b", "complete", 10, 10, 2048),
    ]);
    vi.advanceTimersByTime(300);
    expect(t.success).toHaveBeenCalledWith(
      "Ready offline",
      expect.objectContaining({ id: "offline-download" }),
    );
    expect(isTrackedDownload("a")).toBe(false);
    expect(f.listeners.size).toBe(0);
  });

  it("says what failed", () => {
    const f = fakeManager([rec("a", "queued"), rec("b", "queued")]);
    const open = vi.fn();
    trackDownloadToast(f.manager, ["a", "b"], "Saga", open);
    f.set([rec("a", "complete"), rec("b", "error")]);
    vi.advanceTimersByTime(300);
    expect(t.error).toHaveBeenCalledWith(
      "Saga",
      expect.objectContaining({
        description: "1 ready offline · 1 failed",
        action: { label: "Downloads", onClick: open },
      }),
    );
  });
});
