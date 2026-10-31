// @vitest-environment jsdom
/**
 * The offline shell's reader (audit N-R7 / N-R8): the download record's
 * stored spread overrides reach the reader, and an issue opened outside
 * a list resolves Up Next from the downloaded siblings on the device —
 * never from the network, which is unreachable on the plane.
 */
import "fake-indexeddb/auto";
import * as React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

const captured = vi.hoisted(() => ({ props: [] as Record<string, unknown>[] }));
vi.mock("@/app/[locale]/read/[seriesSlug]/[issueSlug]/Reader", () => ({
  Reader: (props: Record<string, unknown>) => {
    captured.props.push(props);
    return null;
  },
}));

import { OfflineReader } from "@/app/[locale]/downloads/OfflineReader";
import type { PageOverridesView } from "@/lib/api/types";
import {
  createDownloadManager,
  type DownloadManager,
} from "@/lib/pwa/downloads";
import {
  memoryOfflineDb,
  recordKey,
  type DownloadRecord,
  type OfflineDb,
  type OfflineList,
} from "@/lib/pwa/offline-store";

import { asCacheStorage, FakeCacheStorage } from "./fake-caches";

const overrides: PageOverridesView = {
  issue_id: "i1",
  shift_pairing: true,
  spread_pages: [6],
  single_pages: [],
  updated_at: "2026-10-01T00:00:00Z",
};

function rec(
  issueId: string,
  patch: Partial<DownloadRecord> = {},
): DownloadRecord {
  return {
    key: recordKey("alice", issueId),
    account: "alice",
    issueId,
    seriesId: "s1",
    seriesSlug: "saga",
    issueSlug: `issue-${issueId}`,
    seriesName: "Saga",
    title: null,
    number: issueId.slice(1),
    tier: 1080,
    status: "complete",
    error: null,
    errorMessage: null,
    pages: [],
    pageCount: 20,
    donePages: 20,
    missingThumbs: 0,
    bytes: 1_000,
    estimatedBytes: 0,
    contentVersion: null,
    sortNumber: Number(issueId.slice(1)),
    manga: null,
    seriesReadingDirection: null,
    libraryDefaultReadingDirection: null,
    progress: null,
    direct: true,
    createdAt: 1,
    updatedAt: 1,
    completedAt: 1,
    ...patch,
  };
}

let db: OfflineDb;
let fetchSpy: ReturnType<typeof vi.fn>;

async function managerWith(
  records: DownloadRecord[],
): Promise<DownloadManager> {
  for (const r of records) await db.update(r.key, () => r);
  const m = createDownloadManager({
    db,
    caches: asCacheStorage(new FakeCacheStorage()),
    fetch: fetchSpy as unknown as typeof fetch,
    api: fetchSpy as unknown as (path: string) => Promise<Response>,
    online: () => false,
    storage: {
      persisted: vi.fn().mockResolvedValue(true),
      persist: vi.fn().mockResolvedValue(true),
      estimate: vi.fn().mockResolvedValue({ usage: 10, quota: 1000 }),
    } as unknown as StorageManager,
  });
  await m.setAccount("alice");
  return m;
}

async function readerProps(
  current: DownloadRecord,
  manager: DownloadManager,
): Promise<Record<string, unknown>> {
  render(<OfflineReader record={current} manager={manager} />);
  await vi.waitFor(() => expect(captured.props.length).toBeGreaterThan(0));
  return captured.props.at(-1)!;
}

beforeEach(() => {
  captured.props = [];
  db = memoryOfflineDb();
  fetchSpy = vi.fn(async () => {
    throw new TypeError("Failed to fetch");
  });
  vi.stubGlobal("fetch", fetchSpy);
  Object.defineProperty(window.navigator, "onLine", {
    configurable: true,
    get: () => false,
  });
});
afterEach(() => {
  vi.unstubAllGlobals();
  Object.defineProperty(window.navigator, "onLine", {
    configurable: true,
    get: () => true,
  });
});

describe("<OfflineReader> outside a list", () => {
  it("hands the reader the stored spread overrides", async () => {
    const current = rec("i1", { overrides });
    const m = await managerWith([current]);
    const props = await readerProps(current, m);
    expect(props.storedPageOverrides).toEqual(overrides);
  });

  it("a record from before overrides were stored falls back to automatic pairing", async () => {
    const current = rec("i1");
    delete current.overrides;
    const m = await managerWith([current]);
    const props = await readerProps(current, m);
    expect(props.storedPageOverrides).toBeNull();
  });

  it("resolves Up Next to the next downloaded, unfinished sibling without the network", async () => {
    const current = rec("i1");
    const m = await managerWith([
      current,
      // Finished: skipped, as the server's series rule does.
      rec("i2", { progress: { page: 19, finished: true, run: 1 } }),
      // Not complete: not readable offline, skipped.
      rec("i3", { status: "paused", donePages: 4 }),
      rec("i4"),
      // Another series: never a sibling.
      rec("i5", { seriesId: "s2", seriesSlug: "other" }),
      // Earlier in the series: Up Next walks strictly forward.
      rec("i0"),
    ]);
    const props = await readerProps(current, m);
    expect(props.nextUpOverride).toEqual({
      source: "series",
      target: expect.objectContaining({
        id: "i4",
        series_slug: "saga",
        sort_number: 4,
        cover_url: "/issues/i4/pages/0/thumb",
      }),
    });
    // Defined, so the reader's live `useNextUp` query never runs, and
    // the end card's Continue stays inside the offline shell.
    expect(props.nextUpOverride).not.toBeUndefined();
    // Shift+P: sequential, the finished sibling before is still a target.
    expect(props.prevUpOverride).toEqual({
      source: "series",
      target: expect.objectContaining({ id: "i0" }),
    });
    expect(typeof props.onNextIssue).toBe("function");
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("says first issue (not an error) when no downloaded sibling comes before", async () => {
    const current = rec("i1");
    const m = await managerWith([current, rec("i0", { status: "queued" })]);
    const props = await readerProps(current, m);
    expect(props.prevUpOverride).toEqual({ source: "none" });
  });

  it("says caught up (not an error) when no downloaded sibling is left", async () => {
    const current = rec("i2");
    const m = await managerWith([
      current,
      rec("i1"),
      rec("i3", { progress: { page: 19, finished: true, run: 1 } }),
    ]);
    const props = await readerProps(current, m);
    expect(props.nextUpOverride).toEqual({ source: "none" });
    expect(props.prevUpOverride).toEqual({
      source: "series",
      target: expect.objectContaining({ id: "i1" }),
    });
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("orders siblings without a stored sort number by their issue number", async () => {
    const current = rec("i9", { sortNumber: undefined });
    const m = await managerWith([
      current,
      rec("i10", { sortNumber: undefined }),
      rec("i8", { sortNumber: undefined }),
    ]);
    const props = await readerProps(current, m);
    expect(props.nextUpOverride).toEqual({
      source: "series",
      target: expect.objectContaining({ id: "i10" }),
    });
  });
});

describe("<OfflineReader> from a list", () => {
  const list: OfflineList = {
    id: "view-1",
    kind: "cbl",
    sourceId: "cbl-1",
    name: "Crisis",
    tier: 1080,
    entries: [
      { issueId: "i1", position: 0 },
      { issueId: "i2", position: 1 },
      { issueId: "i3", position: 2 },
    ],
    totalInList: 3,
    createdAt: 1,
    updatedAt: 1,
  };

  it("resolves both directions from the list, never the series", async () => {
    const current = rec("i2");
    const m = await managerWith([
      current,
      rec("i1", { progress: { page: 19, finished: true, run: 1 } }),
      rec("i3"),
      // In the series but not in the list: ignored.
      rec("i4"),
    ]);
    render(<OfflineReader record={current} list={list} manager={m} />);
    await vi.waitFor(() => expect(captured.props.length).toBeGreaterThan(0));
    const props = captured.props.at(-1)!;
    expect(props.nextUpOverride).toEqual(
      expect.objectContaining({
        source: "cbl",
        target: expect.objectContaining({ id: "i3" }),
        cbl_position: 3,
        cbl_total: 3,
      }),
    );
    expect(props.prevUpOverride).toEqual(
      expect.objectContaining({
        source: "cbl",
        target: expect.objectContaining({ id: "i1" }),
        cbl_position: 1,
      }),
    );
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
