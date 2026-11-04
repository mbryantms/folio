/**
 * Queue lanes (folio-flair phase 5, F2): merge the per-queue job listing
 * (`GET /admin/queue/jobs`) into three lanes, Queued → Running → Done,
 * filtered by a coarse queue category. Pure, so it's unit-testable.
 * Plan: ~/.claude/plans/folio-flair-phase5-admin.md
 */
import type { JobCard, QueueJobsView } from "@/lib/api/types";

export type QueueFilter = "all" | "scans" | "covers" | "metadata" | "other";

export const QUEUE_FILTERS: { key: QueueFilter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "scans", label: "Scans" },
  { key: "covers", label: "Covers" },
  { key: "metadata", label: "Metadata" },
  { key: "other", label: "Other" },
];

export function queueCategory(queue: string): Exclude<QueueFilter, "all"> {
  if (queue === "scan" || queue === "scan_series") return "scans";
  if (queue === "post_scan_thumbs") return "covers";
  if (queue.startsWith("metadata_") || queue === "entity_enrich")
    return "metadata";
  return "other";
}

export const matchesFilter = (queue: string, filter: QueueFilter) =>
  filter === "all" || queueCategory(queue) === filter;

export type Lanes = {
  /** Waiting jobs, then retry-delayed ones (soonest first). */
  queued: JobCard[];
  running: JobCard[];
  done: JobCard[];
  /** Exact totals (the card lists are capped per queue). */
  counts: {
    waiting: number;
    scheduled: number;
    running: number;
    done: number;
    failed: number;
  };
  /** Exact per-category totals of waiting + scheduled + running, for chips. */
  byFilter: Record<QueueFilter, number>;
};

const t = (iso: string | null | undefined) => (iso ? Date.parse(iso) : 0);

/**
 * Interleave per-queue heads round-robin so one storming queue (2,500
 * cover thumbnails) doesn't push every other queue's next job out of
 * the visible slice.
 */
function roundRobin(lists: JobCard[][]): JobCard[] {
  const out: JobCard[] = [];
  const max = Math.max(0, ...lists.map((l) => l.length));
  for (let i = 0; i < max; i++)
    for (const l of lists) if (l[i]) out.push(l[i]!);
  return out;
}

export function buildLanes(
  view: QueueJobsView | undefined,
  filter: QueueFilter,
): Lanes {
  const byFilter: Record<QueueFilter, number> = {
    all: 0,
    scans: 0,
    covers: 0,
    metadata: 0,
    other: 0,
  };
  const counts = { waiting: 0, scheduled: 0, running: 0, done: 0, failed: 0 };
  const waiting: JobCard[][] = [];
  const scheduled: JobCard[] = [];
  const running: JobCard[] = [];
  const done: JobCard[] = [];
  for (const q of view?.queues ?? []) {
    const pending = q.waiting_count + q.scheduled_count + q.running_count;
    byFilter.all += pending;
    byFilter[queueCategory(q.queue)] += pending;
    if (!matchesFilter(q.queue, filter)) continue;
    counts.waiting += q.waiting_count;
    counts.scheduled += q.scheduled_count;
    counts.running += q.running_count;
    counts.done += q.recent_count;
    counts.failed += q.recent.filter((j) => j.ok === false).length;
    waiting.push(q.waiting);
    scheduled.push(...q.scheduled);
    running.push(...q.running);
    done.push(...q.recent);
  }
  scheduled.sort((a, b) => t(a.next_run_at) - t(b.next_run_at));
  // Executing jobs (with a start time) longest-running first; prefetched
  // ones (no start time yet) after them.
  running.sort((a, b) => {
    if (!a.started_at !== !b.started_at) return a.started_at ? -1 : 1;
    return t(a.started_at) - t(b.started_at);
  });
  done.sort((a, b) => t(b.finished_at) - t(a.finished_at));
  return {
    queued: [...roundRobin(waiting), ...scheduled],
    running,
    done,
    counts,
    byFilter,
  };
}

/** "850ms" / "4.2s" / "3m 07s" / "1h 02m". */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "—";
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)}s`;
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}
