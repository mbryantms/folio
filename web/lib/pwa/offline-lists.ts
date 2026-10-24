/**
 * Offline reading lists — pure helpers over the stored `OfflineList`
 * manifests and `DownloadRecord`s (no network, no React), shared by the
 * offline library, the offline reader's Up Next and their tests.
 */

import type { IssueSummaryView, NextUpView } from "@/lib/api/types";

import {
  offlineCoverKey,
  type DownloadRecord,
  type OfflineList,
} from "./offline-store";

export type OfflineListRow = {
  issueId: string;
  /** 0-based position in the source list. */
  position: number;
  /** Missing when the issue was removed from this device. */
  record: DownloadRecord | undefined;
};

export type OfflineListSummary = {
  /** Issues the list covers on this device. */
  total: number;
  /** Of those, stored completely. */
  downloaded: number;
  /** Of those, finished (per the offline resume position). */
  finished: number;
  bytes: number;
  /** Any issue still queued, downloading, paused or failed. */
  pending: boolean;
};

const byIssue = (
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
): ReadonlyMap<string, DownloadRecord> =>
  records instanceof Map
    ? records
    : new Map(
        (records as readonly DownloadRecord[]).map((r) => [r.issueId, r]),
      );

/** The list's issues in reading order, joined to their records. */
export function offlineListRows(
  list: OfflineList,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
): OfflineListRow[] {
  const map = byIssue(records);
  return [...list.entries]
    .sort((a, b) => a.position - b.position)
    .map((e) => ({
      issueId: e.issueId,
      position: e.position,
      record: map.get(e.issueId),
    }));
}

export function offlineListSummary(
  list: OfflineList,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
): OfflineListSummary {
  const rows = offlineListRows(list, records);
  let downloaded = 0;
  let finished = 0;
  let bytes = 0;
  let pending = false;
  for (const row of rows) {
    const r = row.record;
    if (!r) continue;
    bytes += r.bytes;
    if (r.status === "complete") downloaded++;
    else pending = true;
    if (r.progress?.finished) finished++;
  }
  return { total: rows.length, downloaded, finished, bytes, pending };
}

/**
 * What to read next in a list offline: the first entry after `after`
 * (or from the top when the current issue is not in the list) that is
 * stored completely and not finished — the server's "first unfinished
 * after" rule applied to what is on the device.
 */
export function nextInOfflineList(
  list: OfflineList,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
  after?: string | null,
): (OfflineListRow & { record: DownloadRecord }) | null {
  const rows = offlineListRows(list, records);
  const idx = after ? rows.findIndex((r) => r.issueId === after) : -1;
  for (const row of rows.slice(idx + 1)) {
    const r = row.record;
    if (r && r.status === "complete" && !r.progress?.finished)
      return { ...row, record: r };
  }
  return null;
}

/** The card shape the reader's Up Next surfaces take, from a record. */
export function summaryFromRecord(record: DownloadRecord): IssueSummaryView {
  return {
    id: record.issueId,
    slug: record.issueSlug,
    series_id: record.seriesId,
    series_slug: record.seriesSlug,
    series_name: record.seriesName ?? undefined,
    title: record.title,
    number: record.number,
    sort_number: null,
    year: null,
    page_count: record.pageCount || record.pages.length || null,
    state: "active",
    // The stored cover; the worker serves it to the offline shell.
    cover_url: offlineCoverKey(record.issueId),
    created_at: "",
    updated_at: "",
  };
}

/**
 * The offline reader's Up Next for an issue opened from a list: the
 * same `NextUpView` the server would answer, resolved from the device.
 * `source: "none"` when nothing downloaded is left in the list.
 */
export function offlineNextUp(
  list: OfflineList,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
  currentIssueId: string,
): NextUpView {
  const next = nextInOfflineList(list, records, currentIssueId);
  if (!next) return { source: "none" };
  const position = next.position + 1;
  const total = list.totalInList ?? list.entries.length;
  const target = summaryFromRecord(next.record);
  return list.kind === "cbl"
    ? {
        source: "cbl",
        target,
        cbl_list_id: list.sourceId,
        cbl_list_name: list.name,
        cbl_position: position,
        cbl_total: total,
      }
    : {
        source: "collection",
        target,
        collection_id: list.id,
        collection_name: list.name,
        collection_position: position,
        collection_total: total,
      };
}
