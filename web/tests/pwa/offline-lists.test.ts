/**
 * Offline reading lists — order, "next in list" and the device-resolved
 * Up Next the offline reader shows (no network involved).
 */
import { describe, expect, it } from "vitest";

import {
  nextInOfflineList,
  offlineListRows,
  offlineListSummary,
  offlineNextUp,
} from "@/lib/pwa/offline-lists";
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
    bytes: 1_000,
    estimatedBytes: 0,
    contentVersion: null,
    manga: null,
    seriesReadingDirection: null,
    libraryDefaultReadingDirection: null,
    progress: null,
    direct: false,
    createdAt: 1,
    updatedAt: 1,
    completedAt: 1,
    ...patch,
  };
}

const list: OfflineList = {
  id: "view-1",
  kind: "cbl",
  sourceId: "cbl-1",
  name: "Crisis",
  tier: 1080,
  // Stored out of order on purpose: position is the truth.
  entries: [
    { issueId: "c", position: 7 },
    { issueId: "a", position: 3 },
    { issueId: "b", position: 5 },
    { issueId: "d", position: 9 },
  ],
  totalInList: 12,
  createdAt: 1,
  updatedAt: 1,
};

const records = [
  rec("a", { progress: { page: 19, finished: true, run: 0 } }),
  rec("b"),
  rec("c", { status: "downloading", donePages: 3, bytes: 100 }),
  rec("d"),
];

describe("offline list order", () => {
  it("sorts rows by list position and joins their records", () => {
    const rows = offlineListRows(list, records);
    expect(rows.map((r) => r.issueId)).toEqual(["a", "b", "c", "d"]);
    expect(rows[2]?.record?.status).toBe("downloading");
    expect(offlineListRows(list, [rec("a")])[1]?.record).toBeUndefined();
  });

  it("summarizes downloaded / finished / bytes / pending", () => {
    expect(offlineListSummary(list, records)).toEqual({
      total: 4,
      downloaded: 3,
      finished: 1,
      bytes: 3_100,
      pending: true,
    });
  });

  it("picks the first stored, unfinished entry, from the top or after the current one", () => {
    expect(nextInOfflineList(list, records)?.issueId).toBe("b");
    // After b: c is still downloading → d.
    expect(nextInOfflineList(list, records, "b")?.issueId).toBe("d");
    expect(nextInOfflineList(list, records, "d")).toBeNull();
    // An issue not in the list starts from the top.
    expect(nextInOfflineList(list, records, "zz")?.issueId).toBe("b");
  });
});

describe("offline Up Next", () => {
  it("mirrors the server's CBL answer with list-relative position", () => {
    const view = offlineNextUp(list, records, "a");
    expect(view.source).toBe("cbl");
    expect(view.target?.id).toBe("b");
    expect(view.target?.series_slug).toBe("saga");
    expect(view.target?.slug).toBe("issue-b");
    expect(view.target?.cover_url).toBe("/issues/b/pages/0/thumb");
    expect(view.cbl_list_id).toBe("cbl-1");
    expect(view.cbl_list_name).toBe("Crisis");
    expect(view.cbl_position).toBe(6);
    expect(view.cbl_total).toBe(12);
  });

  it("uses the collection fields for a collection and says none at the end", () => {
    const coll: OfflineList = {
      ...list,
      id: "view-2",
      kind: "collection",
      sourceId: "view-2",
      totalInList: null,
    };
    const view = offlineNextUp(coll, records, "b");
    expect(view.source).toBe("collection");
    expect(view.target?.id).toBe("d");
    expect(view.collection_id).toBe("view-2");
    expect(view.collection_position).toBe(10);
    expect(view.collection_total).toBe(4);
    expect(offlineNextUp(coll, records, "d")).toEqual({ source: "none" });
  });
});
