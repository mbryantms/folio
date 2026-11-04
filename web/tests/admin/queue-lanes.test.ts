import { describe, expect, it } from "vitest";

import {
  buildLanes,
  formatDuration,
  queueCategory,
} from "@/lib/admin/queue-lanes";
import type { JobCard, QueueJobsLane, QueueJobsView } from "@/lib/api/types";

const job = (id: string, over: Partial<JobCard> = {}): JobCard => ({
  task_id: id,
  queue: "post_scan_thumbs",
  state: "waiting",
  label: "Cover thumbnail",
  ...over,
});

const lane = (queue: string, over: Partial<QueueJobsLane>): QueueJobsLane => ({
  queue,
  waiting: [],
  waiting_count: 0,
  scheduled: [],
  scheduled_count: 0,
  running: [],
  running_count: 0,
  recent: [],
  recent_count: 0,
  ...over,
});

const view: QueueJobsView = {
  generated_at: "2026-11-04T00:00:00Z",
  queues: [
    lane("post_scan_thumbs", {
      waiting: [job("t1"), job("t2"), job("t3")],
      waiting_count: 2500,
      running: [
        job("r-late", { state: "running", started_at: "2026-11-04T00:00:09Z" }),
        job("r-pre", { state: "running" }),
        job("r-early", {
          state: "running",
          started_at: "2026-11-04T00:00:01Z",
        }),
      ],
      running_count: 3,
      recent: [
        job("d1", {
          state: "done",
          ok: true,
          finished_at: "2026-11-04T00:00:05Z",
        }),
        job("d2", {
          state: "done",
          ok: false,
          finished_at: "2026-11-04T00:00:07Z",
        }),
      ],
      recent_count: 2,
    }),
    lane("scan", {
      waiting: [job("s1", { queue: "scan" })],
      waiting_count: 1,
      scheduled: [
        job("s-late", {
          queue: "scan",
          state: "scheduled",
          next_run_at: "2026-11-04T01:00:00Z",
        }),
        job("s-soon", {
          queue: "scan",
          state: "scheduled",
          next_run_at: "2026-11-04T00:10:00Z",
        }),
      ],
      scheduled_count: 2,
    }),
  ],
};

describe("queue lanes", () => {
  it("categorises queues", () => {
    expect(queueCategory("scan_series")).toBe("scans");
    expect(queueCategory("post_scan_thumbs")).toBe("covers");
    expect(queueCategory("metadata_apply_issue")).toBe("metadata");
    expect(queueCategory("archive_edit")).toBe("other");
  });

  it("interleaves queue heads, then retries soonest first", () => {
    const l = buildLanes(view, "all");
    expect(l.queued.map((j) => j.task_id)).toEqual([
      "t1",
      "s1",
      "t2",
      "t3",
      "s-soon",
      "s-late",
    ]);
  });

  it("orders running longest first, prefetched last, and done newest first", () => {
    const l = buildLanes(view, "all");
    expect(l.running.map((j) => j.task_id)).toEqual([
      "r-early",
      "r-late",
      "r-pre",
    ]);
    expect(l.done.map((j) => j.task_id)).toEqual(["d2", "d1"]);
  });

  it("filters and keeps exact counts", () => {
    const l = buildLanes(view, "scans");
    expect(l.queued).toHaveLength(3);
    expect(l.running).toHaveLength(0);
    expect(l.counts).toMatchObject({ waiting: 1, scheduled: 2, running: 0 });
    expect(buildLanes(view, "covers").counts.failed).toBe(1);
    expect(l.byFilter).toMatchObject({
      all: 2506,
      covers: 2503,
      scans: 3,
      metadata: 0,
    });
  });

  it("formats durations", () => {
    expect(formatDuration(850)).toBe("850ms");
    expect(formatDuration(4200)).toBe("4.2s");
    expect(formatDuration(187_000)).toBe("3m 07s");
    expect(formatDuration(null)).toBe("—");
  });
});
