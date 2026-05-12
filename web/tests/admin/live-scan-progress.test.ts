import { describe, expect, it } from "vitest";

import { liveScanReducer } from "@/components/admin/library/LiveScanProgress";
import type { ScanEvent } from "@/lib/api/types";

const baseState = {
  scanId: null,
  status: "idle",
  progress: null,
  recentSeries: [],
  health: [],
  severityCounts: { error: 0, warning: 0, info: 0 },
};

describe("liveScanReducer", () => {
  it("resets activity when a new scan starts", () => {
    const prior = {
      ...baseState,
      scanId: "scan-old",
      status: "running",
      recentSeries: ["Old Series"],
      health: [
        {
          kind: "FileAtRoot",
          severity: "warning",
          path: "/x",
          scanId: "scan-old",
        },
      ],
      severityCounts: { error: 0, warning: 1, info: 0 },
    } as Parameters<typeof liveScanReducer>[0];

    const next = liveScanReducer(prior, {
      type: "events",
      events: [
        {
          type: "scan.started",
          library_id: "lib-1",
          scan_id: "scan-new",
          at: new Date().toISOString(),
        },
      ],
    });

    expect(next.scanId).toBe("scan-new");
    expect(next.recentSeries).toEqual([]);
    expect(next.health).toEqual([]);
    expect(next.severityCounts).toEqual({ error: 0, warning: 0, info: 0 });
  });

  it("does not duplicate repeated series or health events", () => {
    const health: ScanEvent = {
      type: "scan.health_issue",
      library_id: "lib-1",
      scan_id: "scan-1",
      kind: "FileAtRoot",
      severity: "warning",
      path: "/library/orphan.cbz",
    };
    const series: ScanEvent = {
      type: "scan.series_updated",
      library_id: "lib-1",
      series_id: "series-1",
      name: "Series One",
    };

    const next = liveScanReducer(
      { ...baseState, scanId: "scan-1", status: "running" } as Parameters<
        typeof liveScanReducer
      >[0],
      { type: "events", events: [series, series, health, health] },
    );

    expect(next.recentSeries).toEqual(["Series One"]);
    expect(next.health).toHaveLength(1);
    expect(next.severityCounts.warning).toBe(1);
  });

  it("keeps planning progress separate from determinate completion", () => {
    const planning: ScanEvent = {
      type: "scan.progress",
      library_id: "lib-1",
      scan_id: "scan-1",
      kind: "library",
      phase: "planning",
      unit: "planning",
      completed: 0,
      total: 1,
      current_label: "Planning scan",
      files_seen: 0,
      files_added: 0,
      files_updated: 0,
      files_unchanged: 0,
      files_skipped: 0,
      files_duplicate: 0,
      issues_removed: 0,
      health_issues: 0,
      series_scanned: 0,
      series_total: 0,
      series_skipped_unchanged: 0,
      files_total: 0,
      root_files: 0,
      empty_folders: 0,
    };

    const next = liveScanReducer(
      baseState as Parameters<typeof liveScanReducer>[0],
      {
        type: "events",
        events: [planning],
      },
    );

    expect(next.status).toBe("running");
    expect(next.progress?.unit).toBe("planning");
    expect(next.progress?.total).toBe(1);
  });

  it("uses scanning progress labels as a recent series fallback", () => {
    const progress: ScanEvent = {
      type: "scan.progress",
      library_id: "lib-1",
      scan_id: "scan-1",
      kind: "library",
      phase: "scanning",
      unit: "work",
      completed: 2,
      total: 8,
      current_label: "Series From Progress",
      files_seen: 1,
      files_added: 1,
      files_updated: 0,
      files_unchanged: 0,
      files_skipped: 0,
      files_duplicate: 0,
      issues_removed: 0,
      health_issues: 0,
      series_scanned: 1,
      series_total: 3,
      series_skipped_unchanged: 0,
      files_total: 5,
      root_files: 0,
      empty_folders: 0,
    };

    const next = liveScanReducer(
      { ...baseState, scanId: "scan-1", status: "running" } as Parameters<
        typeof liveScanReducer
      >[0],
      { type: "events", events: [progress] },
    );

    expect(next.recentSeries).toEqual(["Series From Progress"]);
  });

  it("hydrates health ticker from persisted rows for the active scan", () => {
    const next = liveScanReducer(
      { ...baseState, scanId: "scan-1", status: "completed" } as Parameters<
        typeof liveScanReducer
      >[0],
      {
        type: "healthRows",
        scanId: "scan-1",
        rows: [
          {
            id: "health-1",
            scan_id: "scan-1",
            kind: "FileAtRoot",
            severity: "warning",
            fingerprint: "abc",
            payload: {
              kind: "file_at_root",
              data: { path: "/library/orphan.cbz" },
            },
            first_seen_at: new Date().toISOString(),
            last_seen_at: new Date().toISOString(),
            resolved_at: null,
            dismissed_at: null,
          },
        ],
      },
    );

    expect(next.health).toHaveLength(1);
    expect(next.health[0]?.path).toBe("/library/orphan.cbz");
    expect(next.severityCounts.warning).toBe(1);
  });
});
