import { describe, expect, it } from "vitest";

import {
  buildLibRows,
  doneCount,
  pct,
} from "@/components/admin/library/ScanDashboardClient";
import type { ScanBatchDetailView, ScanEvent } from "@/lib/api/types";

const BATCH = "batch-1";

function member(
  library_id: string,
  library_name: string,
  state: string,
): ScanBatchDetailView["member_runs"][number] {
  return {
    library_id,
    library_name,
    library_slug: library_name.toLowerCase(),
    id: `run-${library_id}`,
    state,
    started_at: "2026-06-04T00:00:00Z",
    ended_at: null,
    stats: {},
    error: null,
    kind: "library",
    series_id: null,
    series_name: null,
    issue_id: null,
    issue_label: null,
  } as ScanBatchDetailView["member_runs"][number];
}

describe("buildLibRows", () => {
  it("seeds rows from member runs, sorted by name", () => {
    const rows = buildLibRows(
      [member("b", "Beta", "queued"), member("a", "Alpha", "queued")],
      [],
      BATCH,
    );
    expect(rows.map((r) => r.name)).toEqual(["Alpha", "Beta"]);
  });

  it("overlays started/progress/completed for the batch", () => {
    const events: ScanEvent[] = [
      {
        type: "scan.started",
        library_id: "a",
        scan_id: "s",
        at: "t",
        batch_id: BATCH,
      },
      {
        type: "scan.progress",
        library_id: "a",
        scan_id: "s",
        kind: "library",
        phase: "scanning",
        unit: "files",
        completed: 3,
        total: 10,
        current_label: "Alpha 003",
        files_seen: 3,
        files_added: 3,
        files_updated: 0,
        files_unchanged: 0,
        files_skipped: 0,
        files_duplicate: 0,
        issues_removed: 0,
        health_issues: 0,
        series_scanned: 1,
        series_total: 1,
        series_skipped_unchanged: 0,
        files_total: 10,
        root_files: 0,
        empty_folders: 0,
      },
    ];
    const a = buildLibRows([member("a", "Alpha", "queued")], events, BATCH)[0]!;
    expect(a.state).toBe("running");
    expect(a.completed).toBe(3);
    expect(a.total).toBe(10);
    expect(a.label).toBe("Alpha 003");
  });

  it("marks complete + failed by batch_id, ignores other batches", () => {
    const events: ScanEvent[] = [
      {
        type: "scan.completed",
        library_id: "a",
        scan_id: "s",
        added: 1,
        updated: 0,
        removed: 0,
        duration_ms: 5,
        batch_id: BATCH,
      },
      {
        type: "scan.failed",
        library_id: "b",
        scan_id: "s2",
        error: "boom",
        batch_id: BATCH,
      },
      // Different batch — must not touch row c.
      {
        type: "scan.completed",
        library_id: "c",
        scan_id: "s3",
        added: 0,
        updated: 0,
        removed: 0,
        duration_ms: 1,
        batch_id: "other",
      },
    ];
    const rows = buildLibRows(
      [
        member("a", "Alpha", "running"),
        member("b", "Beta", "running"),
        member("c", "Gamma", "queued"),
      ],
      events,
      BATCH,
    );
    const byId = Object.fromEntries(rows.map((r) => [r.libraryId, r.state]));
    expect(byId).toEqual({ a: "complete", b: "failed", c: "queued" });
    expect(doneCount(rows)).toBe(2);
  });

  it("progress is matched by library membership even without batch_id", () => {
    // scan.progress carries no batch_id (M8 design); it's matched by lib id.
    const events: ScanEvent[] = [
      {
        type: "scan.progress",
        library_id: "a",
        scan_id: "s",
        kind: "library",
        phase: "scanning",
        unit: "files",
        completed: 5,
        total: 5,
        current_label: null,
        files_seen: 5,
        files_added: 5,
        files_updated: 0,
        files_unchanged: 0,
        files_skipped: 0,
        files_duplicate: 0,
        issues_removed: 0,
        health_issues: 0,
        series_scanned: 1,
        series_total: 1,
        series_skipped_unchanged: 0,
        files_total: 5,
        root_files: 0,
        empty_folders: 0,
      },
    ];
    const a = buildLibRows([member("a", "Alpha", "queued")], events, BATCH)[0]!;
    expect(a.state).toBe("running");
    expect(a.completed).toBe(5);
  });
});

describe("pct", () => {
  it("guards divide-by-zero and clamps to 100", () => {
    expect(pct(0, 0)).toBe(0);
    expect(pct(1, 4)).toBe(25);
    expect(pct(9, 4)).toBe(100);
  });
});
