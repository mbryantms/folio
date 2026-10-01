// @vitest-environment jsdom
/**
 * WP-4.6 eviction UI: sizes and states per download, remove one / remove
 * all behind an AlertDialog confirm, and the offline library's URL →
 * download resolution (a `/read/...` navigation redirected by the worker).
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
}));

import { DownloadsList } from "@/components/offline/DownloadsList";
import { selectedDownload } from "@/app/[locale]/downloads/OfflineLibrary";
import type { DownloadManager } from "@/lib/pwa/downloads";
import type { DownloadRecord } from "@/lib/pwa/offline-store";

function rec(
  issueId: string,
  patch: Partial<DownloadRecord> = {},
): DownloadRecord {
  return {
    key: `alice\u0000${issueId}`,
    account: "alice",
    issueId,
    seriesId: "s",
    seriesSlug: "saga",
    issueSlug: `issue-${issueId}`,
    seriesName: "Saga",
    title: null,
    number: issueId,
    tier: 1080,
    status: "complete",
    error: null,
    errorMessage: null,
    pages: [],
    pageCount: 20,
    donePages: 20,
    missingThumbs: 0,
    bytes: 12_400_000,
    estimatedBytes: 0,
    contentVersion: null,
    manga: null,
    seriesReadingDirection: null,
    libraryDefaultReadingDirection: null,
    progress: null,
    createdAt: 1,
    updatedAt: 1,
    completedAt: 1,
    ...patch,
  };
}

function fakeManager(records: DownloadRecord[]) {
  let snap: readonly DownloadRecord[] = records;
  const listeners = new Set<() => void>();
  const set = (next: DownloadRecord[]) => {
    snap = next;
    listeners.forEach((l) => l());
  };
  const manager = {
    snapshot: () => snap,
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    usage: vi.fn().mockResolvedValue({
      usage: 50_000_000,
      quota: 1_000_000_000,
      persisted: false,
    }),
    remove: vi.fn(async (id: string) =>
      set(snap.filter((r) => r.issueId !== id)),
    ),
    removeAll: vi.fn(async () => set([])),
    pause: vi.fn(),
    resume: vi.fn(),
  } as unknown as DownloadManager;
  return manager;
}

describe("DownloadsList", () => {
  it("shows sizes, progress and quota", async () => {
    const manager = fakeManager([
      rec("1"),
      rec("2", { status: "downloading", donePages: 5, bytes: 3_000_000 }),
      rec("3", {
        status: "error",
        error: "quota",
        errorMessage: "Device storage is full. Remove downloads to free space.",
      }),
    ]);
    render(<DownloadsList manager={manager} />);
    expect(screen.getByText("Downloaded · 12.4 MB")).toBeTruthy();
    expect(screen.getByText(/Downloading · 5 of 20 pages/)).toBeTruthy();
    expect(screen.getByText(/storage is full/i)).toBeTruthy();
    await waitFor(() =>
      expect(screen.getByText(/50\.0 MB used of 1\.0 GB/)).toBeTruthy(),
    );
    expect(
      screen.getByRole("link", { name: "Read" }).getAttribute("href"),
    ).toBe("/downloads?issue=1");
    expect(screen.getByRole("button", { name: "Pause Saga #2" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Retry Saga #3" })).toBeTruthy();
  });

  it("removes one download only after confirmation", async () => {
    const manager = fakeManager([rec("1"), rec("2")]);
    render(<DownloadsList manager={manager} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove Saga #1" }));
    expect(manager.remove).not.toHaveBeenCalled();
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await waitFor(() => expect(manager.remove).toHaveBeenCalledWith("1"));
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Remove Saga #1" }),
      ).toBeNull(),
    );
  });

  it("removes everything after confirmation", async () => {
    const manager = fakeManager([rec("1"), rec("2")]);
    render(<DownloadsList manager={manager} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove all" }));
    expect(await screen.findByText("Remove all downloads?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(manager.removeAll).toHaveBeenCalled());
    expect(await screen.findByText("No downloads")).toBeTruthy();
  });
});

describe("offline library selection", () => {
  const downloads = [rec("1"), rec("2", { status: "paused" })];
  it("resolves ?issue= and the worker's ?from=/read/<s>/<i> redirect", () => {
    expect(
      selectedDownload(downloads, { issue: "1", from: null }).record?.issueId,
    ).toBe("1");
    expect(
      selectedDownload(downloads, { issue: null, from: "/read/saga/issue-2" })
        .record?.issueId,
    ).toBe("2");
  });
  it("reports a requested issue that is not downloaded", () => {
    expect(
      selectedDownload(downloads, { issue: null, from: "/read/saga/issue-9" }),
    ).toEqual({ requested: true, record: undefined });
    expect(
      selectedDownload(downloads, { issue: null, from: "/series/saga" }),
    ).toEqual({ requested: false, record: undefined });
  });
});
