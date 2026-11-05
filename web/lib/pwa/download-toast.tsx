"use client";

import { toast } from "sonner";

import { Progress } from "@/components/ui/progress";
import { formatBytes } from "@/lib/format";

import type { DownloadManager } from "./downloads";
import type { DownloadRecord } from "./offline-store";

/** The one toast a download batch lives in, from "Downloading…" to
 *  "Ready offline". Shared with the per-issue completion toasts in
 *  `offline-bootstrap.ts`, which stand down for tracked issues. */
export const DOWNLOAD_TOAST_ID = "offline-download";

/** Issues whose completion the live batch toast reports. */
const tracked = new Set<string>();

/** Whether a batch toast is reporting this issue (the bootstrap's
 *  per-issue "ready" toast would replace the batch's progress). */
export function isTrackedDownload(issueId: string): boolean {
  return tracked.has(issueId);
}

/** Ids of the downloads a just-started request queued: records that are
 *  now waiting or running and weren't before. (The manager's enqueue
 *  calls don't return ids; a series / list resolves to a count.) */
export function newlyActive(
  before: readonly DownloadRecord[],
  after: readonly DownloadRecord[],
): string[] {
  const was = new Map(before.map((r) => [r.issueId, r.status]));
  return after
    .filter((r) => r.status === "queued" || r.status === "downloading")
    .filter((r) => {
      const prev = was.get(r.issueId);
      return prev !== "queued" && prev !== "downloading";
    })
    .map((r) => r.issueId);
}

export type BatchProgress = {
  total: number;
  complete: number;
  failed: number;
  paused: number;
  bytes: number;
  /** 0–100, by pages where the page counts are known, else by issues. */
  percent: number;
  done: boolean;
};

export function batchProgress(
  records: readonly DownloadRecord[],
  ids: readonly string[],
): BatchProgress {
  const byId = new Map(records.map((r) => [r.issueId, r]));
  let complete = 0;
  let failed = 0;
  let paused = 0;
  let bytes = 0;
  let pages = 0;
  let donePages = 0;
  let unknownPages = 0;
  for (const id of ids) {
    const r = byId.get(id);
    // Removed mid-batch: nothing left to wait for.
    if (!r) {
      paused++;
      continue;
    }
    bytes += r.bytes;
    if (r.status === "complete") complete++;
    else if (r.status === "error") failed++;
    else if (r.status === "paused") paused++;
    const count = r.pageCount || r.pages.length;
    if (count > 0) {
      pages += count;
      donePages += r.status === "complete" ? count : r.donePages;
    } else unknownPages++;
  }
  const total = ids.length;
  const settled = complete + failed + paused;
  const percent =
    total === 0
      ? 100
      : unknownPages === 0 && pages > 0
        ? (donePages / pages) * 100
        : (settled / total) * 100;
  return {
    total,
    complete,
    failed,
    paused,
    bytes,
    percent: Math.round(Math.min(100, Math.max(0, percent))),
    done: settled === total,
  };
}

function progressLine(p: BatchProgress): string {
  const issues = p.total === 1 ? "" : `${p.complete} of ${p.total} issues · `;
  return `${issues}${formatBytes(p.bytes)}`;
}

/**
 * Keep the batch's "Downloading…" toast up to date until every issue in
 * `ids` has settled, then finish it as "Ready offline" (or say what
 * failed). Lives outside the dialog — the dialog closes as soon as the
 * download starts. Returns a stop function.
 */
export function trackDownloadToast(
  manager: DownloadManager,
  ids: readonly string[],
  label: string,
  /** "Downloads" action on a partial / failed finish (the caller's
   *  router — this outlives the dialog that started it). */
  openDownloads?: () => void,
): () => void {
  if (ids.length === 0) return () => {};
  for (const id of ids) tracked.add(id);
  let last = "";
  let timer: ReturnType<typeof setTimeout> | null = null;
  let stopped = false;

  const stop = () => {
    if (stopped) return;
    stopped = true;
    off();
    if (timer) clearTimeout(timer);
    for (const id of ids) tracked.delete(id);
  };

  const render = () => {
    timer = null;
    if (stopped) return;
    const p = batchProgress(manager.snapshot(), ids);
    if (p.done) {
      stop();
      if (p.complete === p.total) {
        toast.success("Ready offline", {
          id: DOWNLOAD_TOAST_ID,
          description: `${label} · ${formatBytes(p.bytes)}`,
        });
      } else if (p.complete === 0 && p.failed === 0) {
        toast.message("Download paused", {
          id: DOWNLOAD_TOAST_ID,
          description: label,
        });
      } else {
        const bits = [
          p.complete > 0 ? `${p.complete} ready offline` : null,
          p.failed > 0 ? `${p.failed} failed` : null,
          p.paused > 0 ? `${p.paused} paused` : null,
        ].filter(Boolean);
        (p.failed > 0 ? toast.error : toast.message)(label, {
          id: DOWNLOAD_TOAST_ID,
          description: bits.join(" · "),
          ...(openDownloads
            ? { action: { label: "Downloads", onClick: openDownloads } }
            : {}),
        });
      }
      return;
    }
    const line = progressLine(p);
    const key = `${p.percent}|${line}`;
    if (key === last) return;
    last = key;
    toast.loading(`Downloading ${label}`, {
      id: DOWNLOAD_TOAST_ID,
      duration: Infinity,
      description: (
        <span className="flair-download-toast">
          <Progress
            value={p.percent}
            aria-label={`Download progress, ${p.percent}%`}
          />
          <span className="text-muted-foreground text-xs tabular-nums">
            {p.percent}% · {line}
          </span>
        </span>
      ),
    });
  };

  // The manager notifies per stored page; four updates a second is
  // plenty for a progress bar and keeps sonner from re-rendering on
  // every page.
  const schedule = () => {
    if (!timer && !stopped) timer = setTimeout(render, 250);
  };
  const off = manager.subscribe(schedule);
  render();
  return stop;
}
