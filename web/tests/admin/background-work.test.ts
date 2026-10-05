import { describe, expect, it } from "vitest";

import {
  compactCount,
  otherWork,
  pct,
  pillBreakdown,
  queueHref,
  sortLibraries,
} from "@/lib/admin/background-work";
import type { BackgroundWorkView, LibraryWorkView } from "@/lib/api/types";

const q = (queue: string, waiting: number, in_flight = 0, scheduled = 0) => ({
  queue,
  waiting,
  scheduled,
  in_flight,
});

function lib(over: Partial<LibraryWorkView>): LibraryWorkView {
  return {
    id: "id",
    slug: "slug",
    name: "Lib",
    scan: null,
    scoped_scans: 0,
    issues_total: 0,
    covers_ready: 0,
    covers_remaining: 0,
    covers_hash_only: 0,
    covers_errored: 0,
    cover_jobs_queued: 0,
    cover_jobs_running: 0,
    page_jobs_queued: 0,
    page_jobs_running: 0,
    hash_pending: 0,
    busy: false,
    ...over,
  };
}

const scan = (state: string): NonNullable<LibraryWorkView["scan"]> => ({
  id: "run",
  state,
  kind: "library",
  started_at: "2026-10-05T00:00:00Z",
  batch_id: null,
  phase: null,
  completed: null,
  total: null,
  current_label: null,
  files_per_sec: null,
  stalled: false,
});

describe("compactCount", () => {
  it("keeps small numbers exact and abbreviates large ones", () => {
    expect(compactCount(0)).toBe("0");
    expect(compactCount(999)).toBe("999");
    expect(compactCount(1000)).toBe("1.0k");
    expect(compactCount(1250)).toBe("1.2k");
    expect(compactCount(16_204)).toBe("16k");
    expect(compactCount(2_340_000)).toBe("2.3M");
  });
});

describe("pillBreakdown", () => {
  it("names the largest kinds of work, merging queues an operator sees as one", () => {
    const text = pillBreakdown({
      queues: [
        q("post_scan_thumbs", 14_000, 200),
        q("scan", 2, 1),
        q("scan_series", 1),
        q("metadata_search_issue", 100, 0, 20),
        q("archive_edit", 0),
      ],
    });
    expect(text).toBe("14,200 thumbnails · 120 metadata · 4 scans");
  });

  it("folds everything past the cap into 'other'", () => {
    const text = pillBreakdown(
      {
        queues: [
          q("post_scan_thumbs", 50),
          q("scan", 5),
          q("backfill", 2),
          q("archive_edit", 1),
        ],
      },
      2,
    );
    expect(text).toBe("50 thumbnails · 5 scans · 3 other");
  });

  it("is empty when nothing is pending", () => {
    expect(pillBreakdown({ queues: [q("scan", 0)] })).toBe("");
  });
});

describe("pct", () => {
  it("clamps and tolerates a zero total", () => {
    expect(pct(0, 0)).toBe(0);
    expect(pct(5, 10)).toBe(50);
    expect(pct(12, 10)).toBe(100);
  });
});

describe("sortLibraries", () => {
  it("orders scanning, queued, thumbnail-only, then idle; names break ties", () => {
    const sorted = sortLibraries([
      lib({ name: "Idle B" }),
      lib({ name: "Thumbs", busy: true }),
      lib({ name: "Queued", busy: true, scan: scan("queued") }),
      lib({ name: "Idle A" }),
      lib({ name: "Running", busy: true, scan: scan("running") }),
    ]);
    expect(sorted.map((l) => l.name)).toEqual([
      "Running",
      "Queued",
      "Thumbs",
      "Idle A",
      "Idle B",
    ]);
  });
});

describe("otherWork", () => {
  it("lists only non-empty queues the library table does not cover", () => {
    const view = {
      queues: [
        { ...q("scan", 3), dead: 0 },
        { ...q("post_scan_thumbs", 900), dead: 0 },
        { ...q("metadata_apply_issue", 4, 1), dead: 0 },
        { ...q("archive_edit", 0), dead: 2 },
        { ...q("hash_backfill", 0, 1), dead: 0 },
      ],
    } as unknown as BackgroundWorkView;
    expect(otherWork(view)).toEqual([
      {
        queue: "metadata_apply_issue",
        label: "Metadata apply (issue)",
        href: "/admin/metadata",
        pending: 5,
        inFlight: 1,
      },
      {
        queue: "hash_backfill",
        label: "Content hashing",
        href: "/admin/queue",
        pending: 1,
        inFlight: 1,
      },
    ]);
  });
});

describe("queueHref", () => {
  it("sends metadata work to the metadata page and the rest to the queue page", () => {
    expect(queueHref("metadata_search_series")).toBe("/admin/metadata");
    expect(queueHref("provider_coverage")).toBe("/admin/metadata");
    expect(queueHref("rewrite_issue_sidecars")).toBe("/admin/queue");
  });
});
