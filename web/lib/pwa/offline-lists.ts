/**
 * Offline reading lists — pure helpers over the stored `OfflineList`
 * manifests and `DownloadRecord`s (no network, no React), shared by the
 * offline library, the offline reader's Up Next (list and series
 * fallback) and their tests.
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
    sort_number: record.sortNumber ?? null,
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
 * The entry before `before` in the list that is stored completely —
 * sequential back-navigation with no finished filter, the server's
 * prev-up rule. `null` at the top of the list, or when `before` is not
 * in it.
 */
export function prevInOfflineList(
  list: OfflineList,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
  before: string,
): (OfflineListRow & { record: DownloadRecord }) | null {
  const rows = offlineListRows(list, records);
  const idx = rows.findIndex((r) => r.issueId === before);
  for (const row of rows.slice(0, Math.max(idx, 0)).reverse()) {
    const r = row.record;
    if (r && r.status === "complete") return { ...row, record: r };
  }
  return null;
}

/** The `NextUpView` for one list row: the server's CBL / collection
 *  shape with the list-relative position and total. */
function listView(
  list: OfflineList,
  row: OfflineListRow & { record: DownloadRecord },
): NextUpView {
  const position = row.position + 1;
  const total = list.totalInList ?? list.entries.length;
  const target = summaryFromRecord(row.record);
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
  return next ? listView(list, next) : { source: "none" };
}

/** The offline reader's previous issue in a list (`Shift+P`), resolved
 *  from the device like `offlineNextUp`; `source: "none"` at the top. */
export function offlinePrevUp(
  list: OfflineList,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
  currentIssueId: string,
): NextUpView {
  const prev = prevInOfflineList(list, records, currentIssueId);
  return prev ? listView(list, prev) : { source: "none" };
}

/** Numeric key for series order: `sort_number` when the record carries
 *  it, else a numeric reading of `number`; `null` sorts last. */
function seriesOrderKey(r: DownloadRecord): number | null {
  if (typeof r.sortNumber === "number") return r.sortNumber;
  const n = r.number ? Number.parseFloat(r.number) : Number.NaN;
  return Number.isFinite(n) ? n : null;
}

/** The series-page order the server walks (`sort_number` ascending,
 *  nulls last, then issue id), applied to download records. */
function compareSeriesOrder(a: DownloadRecord, b: DownloadRecord): number {
  const ka = seriesOrderKey(a);
  const kb = seriesOrderKey(b);
  if (ka !== null && kb !== null && ka !== kb) return ka - kb;
  if ((ka === null) !== (kb === null)) return ka === null ? 1 : -1;
  return a.issueId < b.issueId ? -1 : a.issueId > b.issueId ? 1 : 0;
}

/**
 * The offline reader's Up Next for an issue opened on its own, not from
 * a list: the server's series rule applied to what is on the device —
 * the first downloaded, unfinished sibling strictly after the current
 * issue in series order (audit N-R8). `source: "none"` when nothing
 * downloaded is left, so the end card says caught up rather than
 * failing on the network.
 */
export function offlineSeriesNextUp(
  current: DownloadRecord,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
): NextUpView {
  const { ordered, idx } = seriesOrdered(current, records);
  for (const r of ordered.slice(idx + 1))
    if (r.status === "complete" && !r.progress?.finished)
      return { source: "series", target: summaryFromRecord(r) };
  return { source: "none" };
}

/** The previous downloaded sibling in series order — sequential, no
 *  finished filter, the server's prev-up rule — or `source: "none"` at
 *  the first downloaded issue. */
export function offlineSeriesPrevUp(
  current: DownloadRecord,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
): NextUpView {
  const { ordered, idx } = seriesOrdered(current, records);
  for (const r of ordered.slice(0, idx).reverse())
    if (r.status === "complete")
      return { source: "series", target: summaryFromRecord(r) };
  return { source: "none" };
}

/** The current issue with its downloaded siblings in series order, and
 *  its index among them. */
function seriesOrdered(
  current: DownloadRecord,
  records: readonly DownloadRecord[] | ReadonlyMap<string, DownloadRecord>,
): { ordered: DownloadRecord[]; idx: number } {
  const ordered = [
    ...[...byIssue(records).values()].filter(
      (r) => r.seriesId === current.seriesId && r.issueId !== current.issueId,
    ),
    current,
  ].sort(compareSeriesOrder);
  return {
    ordered,
    idx: ordered.findIndex((r) => r.issueId === current.issueId),
  };
}
