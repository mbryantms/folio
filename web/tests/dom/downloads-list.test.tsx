// @vitest-environment jsdom
/**
 * WP-4.6 eviction UI: sizes and states per download, remove one / remove
 * all behind an AlertDialog confirm, and the offline library's URL →
 * download resolution (a `/read/...` navigation redirected by the worker).
 */
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(),
}));

import { DownloadsList } from "@/components/offline/DownloadsList";
import { selectedDownload } from "@/app/[locale]/downloads/OfflineLibrary";
import type { DownloadManager } from "@/lib/pwa/downloads";
import type { DownloadRecord, OfflineList } from "@/lib/pwa/offline-store";

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

function fakeManager(records: DownloadRecord[], lists: OfflineList[] = []) {
  let snap: readonly DownloadRecord[] = records;
  let listSnap: readonly OfflineList[] = lists;
  const listeners = new Set<() => void>();
  const set = (next: DownloadRecord[]) => {
    snap = next;
    listeners.forEach((l) => l());
  };
  const manager = {
    snapshot: () => snap,
    lists: () => listSnap,
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    removeList: vi.fn(async (id: string) => {
      const list = listSnap.find((l) => l.id === id);
      listSnap = listSnap.filter((l) => l.id !== id);
      set(
        snap.filter((r) => !list?.entries.some((e) => e.issueId === r.issueId)),
      );
    }),
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

describe("DownloadsList with offline reading lists", () => {
  const crisis: OfflineList = {
    id: "view-1",
    kind: "cbl",
    sourceId: "L1",
    name: "Crisis on Infinite Earths",
    tier: 1080,
    // Stored out of order: the section follows list position.
    entries: [
      { issueId: "7", position: 6 },
      { issueId: "5", position: 4 },
      { issueId: "6", position: 5 },
    ],
    totalInList: 12,
    createdAt: 1,
    updatedAt: 1,
  };
  const records = [
    rec("1"),
    rec("5", { progress: { page: 19, finished: true, run: 0 } }),
    rec("6"),
    rec("7", { status: "queued", donePages: 0, bytes: 0 }),
  ];

  it("keeps a list together, in reading order, apart from other downloads", () => {
    const manager = fakeManager(records, [crisis]);
    render(<DownloadsList manager={manager} />);
    const section = screen.getByRole("region", {
      name: "Crisis on Infinite Earths",
    });
    expect(within(section).getByText("Reading list")).toBeTruthy();
    expect(
      within(section).getByText(
        /2 of 3 issues downloaded · 1 read · .* · 12 in the full list/,
      ),
    ).toBeTruthy();
    const labels = within(section)
      .getAllByRole("listitem")
      .map((li) => li.textContent ?? "");
    expect(labels[0]).toMatch(/^5Saga #5Read/);
    expect(labels[1]).toMatch(/^6Saga #6/);
    expect(labels[2]).toMatch(/^7Saga #7/);
    // Continue lands on the first unfinished downloaded entry, with the
    // list carried into the offline reader.
    expect(
      within(section)
        .getByRole("link", { name: /^Continue Crisis on Infinite Earths/ })
        .getAttribute("href"),
    ).toBe("/downloads?issue=6&list=view-1");
    expect(
      within(section)
        .getAllByRole("link", { name: "Read" })
        .map((a) => a.getAttribute("href")),
    ).toEqual([
      "/downloads?issue=5&list=view-1",
      "/downloads?issue=6&list=view-1",
    ]);
    // The lone issue is segmented under its own heading, without a list.
    const others = screen.getByRole("region", { name: "Other downloads" });
    expect(within(others).getAllByRole("listitem")).toHaveLength(1);
    expect(
      within(others).getByRole("link", { name: "Read" }).getAttribute("href"),
    ).toBe("/downloads?issue=1");
    expect(screen.getByText(/in 4 downloads · 1 list/)).toBeTruthy();
  });

  it("removes a whole list only after confirmation", async () => {
    const manager = fakeManager(records, [crisis]);
    render(<DownloadsList manager={manager} />);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Remove list Crisis on Infinite Earths",
      }),
    );
    expect(
      await screen.findByText(
        "Remove Crisis on Infinite Earths from this device?",
      ),
    ).toBeTruthy();
    expect(manager.removeList).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(manager.removeList).toHaveBeenCalledWith("view-1"),
    );
    await waitFor(() =>
      expect(
        screen.queryByRole("region", { name: "Crisis on Infinite Earths" }),
      ).toBeNull(),
    );
    // What was downloaded on its own stays.
    expect(screen.getByRole("button", { name: "Remove Saga #1" })).toBeTruthy();
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
